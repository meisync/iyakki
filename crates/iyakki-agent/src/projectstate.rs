//! The state of a workspace, written by the harness from the disk at the start of every run: the detected stack, the feature board,
//! open bugs, recently changed files, the last deploy and the last session. It is added to the model's context so that no run (and no new
//! chat of the same workspace) starts from the beginning, and it also decides which skills are always included.
//!
//! Nothing here comes from the model's memory: every call reads the files again.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::featureboard::{BoardFile, Feature};

/// What the project is made of and how it is run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stack {
    /// Language names, the main one first ("Go", "TypeScript", "C++").
    pub names: Vec<String>,
    /// The `lang-*` skill of the main language, when one exists.
    pub lang_skill: Option<&'static str>,
    pub run: String,
    pub test: String,
    pub build: String,
    pub dockerfile: bool,
    pub deploy_dir: bool,
    /// The kind in an `iyakki-app.json` of the root or a folder below it (a project that extends IYAKKI): pins the authoring skills.
    pub app_kind: Option<String>,
    /// What the workspace is for (`.iyakki/project.json`), when it was chosen.
    pub target: Option<crate::target::Target>,
}

impl Stack {
    pub fn label(&self) -> String {
        if self.names.is_empty() { "unknown".into() } else { self.names.join(" + ") }
    }

    /// The skills that are always included for this project: its language, and the k3s practices when it ships a container.
    pub fn pinned_skills(&self, request: &str) -> Vec<&'static str> {
        let mut v = Vec::new();
        if let Some(s) = self.lang_skill {
            v.push(s);
        }
        let r = request.to_lowercase();
        use crate::target::{resolve_intent, Intent, Target};
        let tile = self.target == Some(Target::IyakkiTile) || (self.target.is_none() && resolve_intent(request) == Intent::Is(Target::IyakkiTile) && !request.trim().is_empty());
        if tile {
            v.push("iyakki-app-author");
            v.push("iyakki-tile-author");
        }
        let wants_app = ["new app", "custom app", "mcp server", "iyakki skill", "new skill", "publish to the store", "private store"].iter().any(|w| r.contains(w));
        if (self.app_kind.is_some() || wants_app) && !v.contains(&"iyakki-app-author") {
            v.push("iyakki-app-author");
            match self.app_kind.as_deref() {
                Some("tile") if !v.contains(&"iyakki-tile-author") => v.push("iyakki-tile-author"),
                Some("mcp") => v.push("iyakki-mcp-author"),
                Some("skill") => v.push("iyakki-skill-author"),
                None if r.contains("mcp") => v.push("iyakki-mcp-author"),
                None if r.contains("skill") => v.push("iyakki-skill-author"),
                _ => {}
            }
        }
        let k3s_ok = self.target.is_none_or(Target::deploys_to_k3s) && !tile;
        if k3s_ok && (self.dockerfile || self.deploy_dir || ["deploy", "k3s", "kubernetes", "k8s", "cluster", "container"].iter().any(|w| r.contains(w))) {
            v.push("k3s-dev-best-practices");
        }
        v
    }
}

fn lang(name: &str) -> Option<(&'static str, &'static str, [&'static str; 3])> {
    // (display name, lang skill, [run, test, build])
    Some(match name.to_lowercase().trim() {
        "go" | "golang" => ("Go", "lang-go", ["go run .", "go test ./...", "go build ./..."]),
        "javascript" | "js" | "node" | "nodejs" | "node.js" => ("JavaScript", "lang-javascript", ["npm start", "npm test", "npm run build"]),
        "typescript" | "ts" => ("TypeScript", "lang-typescript", ["npm start", "npm test", "npm run build"]),
        "python" | "py" => ("Python", "lang-python", ["python3 main.py", "python3 -m pytest", ""]),
        "java" | "kotlin" => ("Java", "lang-java", ["mvn exec:java", "mvn test", "mvn package"]),
        "rust" | "rs" => ("Rust", "lang-rust", ["cargo run", "cargo test", "cargo build"]),
        "c#" | "csharp" | "dotnet" | ".net" => ("C#", "lang-csharp", ["dotnet run", "dotnet test", "dotnet build"]),
        "php" => ("PHP", "lang-php", ["php -S localhost:8000", "vendor/bin/phpunit", ""]),
        "c++" | "cpp" => ("C++", "lang-cpp", ["make run", "make test", "make"]),
        "c" => ("C", "lang-c", ["make run", "make test", "make"]),
        _ => return None,
    })
}

fn has_ext(dir: &Path, exts: &[&str], depth: usize) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten().take(400) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_file() && exts.iter().any(|x| name.ends_with(x)) {
            return true;
        }
        if p.is_dir() && depth > 0 && !name.starts_with('.') && !matches!(name.as_str(), "node_modules" | "target" | "vendor" | "build" | "dist") && has_ext(&p, exts, depth - 1) {
            return true;
        }
    }
    false
}

/// Are all names of a stack setting ones we know (`Go`, `TypeScript + Python`)?
pub fn is_known_stack(setting: &str) -> bool {
    setting.split(['+', ',']).map(str::trim).filter(|n| !n.is_empty()).all(|n| lang(n).is_some())
}

/// The stack from the files of the project (the root and one folder down), unless `override_name` names one.
pub fn detect_stack(root: &Path, override_name: &str) -> Stack {
    let mut st = Stack::default();
    st.dockerfile = root.join("Dockerfile").exists() || root.join("Containerfile").exists() || std::fs::read_dir(root).map(|rd| rd.flatten().any(|e| e.path().join("Dockerfile").exists())).unwrap_or(false);
    st.target = crate::target::read(root);
    st.deploy_dir = root.join("deploy").is_dir() && has_ext(&root.join("deploy"), &[".yaml", ".yml"], 1);
    st.app_kind = std::iter::once(root.to_path_buf()).chain(std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir())).take(40).find_map(|d| crate::store::vet::read_project(&d)).map(|v| v["kind"].as_str().unwrap_or("").to_string());
    let mut found: Vec<&str> = Vec::new();
    for name in override_name.split(['+', ',']).map(str::trim).filter(|n| !n.is_empty()) {
        if lang(name).is_some() {
            found.push(name);
        }
    }
    if found.is_empty() {
        // the root and the folders just below it
        let mut dirs = vec![root.to_path_buf()];
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.') || matches!(n.to_string_lossy().as_ref(), "node_modules" | "target" | "vendor" | "dist" | "build"))).collect();
            subs.sort();
            dirs.extend(subs.into_iter().take(20));
        }
        for d in dirs {
            let f = |n: &str| d.join(n).exists();
            let mut add = |l: &'static str| {
                if !found.contains(&l) {
                    found.push(l);
                }
            };
            if f("go.mod") {
                add("go");
            }
            if f("tsconfig.json") || (f("package.json") && std::fs::read_to_string(d.join("package.json")).is_ok_and(|t| t.contains("\"typescript\""))) {
                add("typescript");
            } else if f("package.json") {
                add("javascript");
            }
            if f("requirements.txt") || f("pyproject.toml") || f("setup.py") || f("Pipfile") {
                add("python");
            }
            if f("pom.xml") || f("build.gradle") || f("build.gradle.kts") {
                add("java");
            }
            if f("Cargo.toml") {
                add("rust");
            }
            if f("composer.json") {
                add("php");
            }
            if std::fs::read_dir(&d).map(|rd| rd.flatten().any(|e| { let n = e.file_name().to_string_lossy().to_string(); n.ends_with(".csproj") || n.ends_with(".sln") })).unwrap_or(false) {
                add("c#");
            }
            if f("CMakeLists.txt") || f("Makefile") {
                if has_ext(&d, &[".cpp", ".cc", ".cxx", ".hpp"], 2) {
                    add("c++");
                } else if has_ext(&d, &[".c", ".h"], 2) {
                    add("c");
                }
            }
        }
    }
    for n in &found {
        if let Some((display, skill, cmds)) = lang(n) {
            if st.names.iter().any(|x| x == display) {
                continue;
            }
            if st.names.is_empty() {
                st.lang_skill = Some(skill);
                st.run = cmds[0].into();
                st.test = cmds[1].into();
                st.build = cmds[2].into();
                // a Maven project with Gradle files: the Gradle commands
                if display == "Java" && !root.join("pom.xml").exists() && root.join("build.gradle").exists() {
                    st.test = "gradle test".into();
                    st.build = "gradle build".into();
                    st.run = "gradle run".into();
                }
            }
            st.names.push(display.to_string());
        }
    }
    st
}

// ------------------------------------------------------------------------------------------------ the deploy state file

fn state_path(root: &Path) -> PathBuf {
    root.join(".iyakki/state.json")
}

fn read_state(root: &Path) -> Value {
    std::fs::read_to_string(state_path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

/// One remembered fact (empty when none).
pub fn noted(root: &Path, key: &str) -> Option<String> {
    read_state(root).get(key).and_then(Value::as_str).map(str::to_string).filter(|v| !v.is_empty())
}

/// Remember one fact of the last deploy (image, status headline, reachable address…), for the next run's state block.
pub fn note(root: &Path, key: &str, value: &str) {
    let mut v = read_state(root);
    if !v.is_object() {
        v = json!({});
    }
    v[key] = json!(crate::agent::truncate(value, 300));
    v["updated"] = json!(crate::clock::now_unix());
    let _ = std::fs::create_dir_all(root.join(".iyakki"));
    let _ = std::fs::write(state_path(root), v.to_string());
}

// ------------------------------------------------------------------------------------------------ the block

/// Everything the block is built from.
pub struct Input<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    pub features: &'a Path,
    /// From project.json: `stack`, and the test/run/build commands.
    pub stack_override: &'a str,
    pub test: &'a str,
    pub run: &'a str,
    pub build: &'a str,
    /// "k3s · namespace shop-dev" when a cluster is connected.
    pub cluster: Option<String>,
    pub history: Option<&'a crate::history::History>,
}

const MAX_CHARS: usize = 3600;

fn short_time(stamp: &str) -> String {
    stamp.split_once(' ').map(|x| x.1.to_string()).unwrap_or_else(|| stamp.to_string())
}

fn row_line(f: &Feature, results: Option<&crate::testresults::Results>) -> String {
    let mut s = format!("{} [{}] {}", f.id, f.status, crate::agent::truncate(&f.title, 50).replace('\n', " "));
    if !f.started.is_empty() {
        s.push_str(&format!(" · started {}", short_time(&f.started)));
    }
    if let (Some(r), true) = (results, !f.tests.is_empty()) {
        let m = crate::testresults::match_row(&f.tests, r);
        s.push_str(&format!(" · {} tests {}", f.tests.len(), m.label()));
    } else if !f.tests.is_empty() {
        s.push_str(&format!(" · {} tests", f.tests.len()));
    }
    if f.status == "pending" && !f.depends().is_empty() {
        s.push_str(&format!(" · waits for {}", crate::agent::truncate(&f.depends(), 40)));
    }
    let note: String = f.notes.split(" · auto:").next().unwrap_or("").chars().take(60).collect();
    let note = if note.is_empty() { f.notes.split("auto:").nth(1).unwrap_or("").trim().chars().take(60).collect() } else { note };
    if !note.trim().is_empty() {
        s.push_str(&format!(" · {}", note.trim()));
    }
    s
}

fn recent_files(root: &Path, max: usize) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(SystemTime, String)>, budget: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || matches!(name.as_str(), "node_modules" | "target" | "vendor" | "dist" | "build" | "__pycache__") {
                continue;
            }
            if p.is_dir() {
                if depth > 0 {
                    walk(root, &p, depth - 1, out, budget);
                }
            } else if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                out.push((m, p.strip_prefix(root).map(|r| r.display().to_string()).unwrap_or(name)));
            }
        }
    }
    let mut out = Vec::new();
    let mut budget = 3000;
    walk(root, root, 4, &mut out, &mut budget);
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(7 * 86400)).unwrap_or(SystemTime::UNIX_EPOCH);
    out.retain(|(m, f)| *m >= cutoff && f != "FEATURES.md");
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().take(max).map(|x| x.1).collect()
}

/// The PROJECT STATE block, at most about 900 tokens: sections are dropped from the least important end when the board is large.
pub fn build(inp: &Input) -> String {
    build_with_notes(inp, inp.root)
}

/// Like [`build`], with the workspace notes read from `notes_root`: a task works in a copy of the workspace, its notes are the workspace's.
/// The notes come right after the first line, in their own token budget: the cut that keeps the block short never touches them.
pub fn build_with_notes(inp: &Input, notes_root: &Path) -> String {
    let stack = detect_stack(inp.root, inp.stack_override);
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("PROJECT STATE (written by the harness from the files just now; the workspace \"{}\" at {}):", inp.name, inp.root.display()));
    let cmd = |given: &str, default: &str| if given.trim().is_empty() { default.to_string() } else { given.to_string() };
    let service = stack.target.is_none_or(crate::target::Target::deploys_to_k3s);
    parts.push(format!(
        "Stack: {}{}. Commands: run `{}`, test `{}`{}. {}{}",
        stack.label(),
        if inp.stack_override.trim().is_empty() { "" } else { " (set in project.json)" },
        cmd(inp.run, &stack.run),
        cmd(inp.test, &stack.test),
        { let b = cmd(inp.build, &stack.build); if b.is_empty() { String::new() } else { format!(", build `{b}`") } },
        if stack.dockerfile && service { "A Dockerfile exists. " } else { "" },
        if stack.deploy_dir && service { "Manifests exist in deploy/." } else { "" }
    ));
    parts.push(crate::target::state_line(inp.root));
    // the board
    let results = crate::testresults::load(inp.root);
    let mut graph: Option<crate::boarddeps::Graph> = None;
    let mut rows_text = String::new();
    let mut bugs_text = String::new();
    let counts: String;
    if let Ok(bf) = BoardFile::load(inp.features) {
        let all = bf.board.features();
        let (bugs, feats): (Vec<&Feature>, Vec<&Feature>) = all.iter().partition(|f| f.is_bug());
        let done = feats.iter().filter(|f| f.status == "done").count();
        counts = format!("Feature board ({}): {} rows, {} done, {} in progress or waiting.", inp.features.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), feats.len(), done, feats.iter().filter(|f| f.status != "done" && f.status != "deprecated").count());
        graph = Some(crate::boarddeps::Graph::build(&all));
        let rank = |s: &str| match s { "partial" => 0, "review" => 1, "pending" => 2, "blocked" => 3, "planned" => 4, _ => 5 };
        let mut open: Vec<&&Feature> = feats.iter().filter(|f| f.status != "done" && f.status != "deprecated").collect();
        open.sort_by_key(|f| (rank(&f.status), f.id.clone()));
        let shown = open.len().min(14);
        for f in open.iter().take(shown) {
            rows_text.push_str(&format!("- {}{}\n", row_line(f, results.as_ref()), graph.as_ref().map(|g| crate::boarddeps::state_suffix(g, &f.id)).unwrap_or_default()));
        }
        if open.len() > shown {
            rows_text.push_str(&format!("- … {} more unfinished rows (board_list shows them)\n", open.len() - shown));
        }
        let open_bugs: Vec<&&Feature> = bugs.iter().filter(|b| b.status != "done").collect();
        if !open_bugs.is_empty() {
            bugs_text.push_str(&format!("Open bugs ({}):\\n", open_bugs.len()).replace("\\n", "\n"));
            for b in open_bugs.iter().take(5) {
                bugs_text.push_str(&format!("- {} [{}] {} ({}){}\n", b.id, b.status, crate::agent::truncate(&b.title, 60), if b.severity.is_empty() { "-" } else { &b.severity }, b.logs.first().map(|l| format!(" logs {l}")).unwrap_or_default()));
            }
        }
    } else {
        counts = "Feature board: none yet (board_add creates it).".into();
    }
    parts.push(counts);
    if !rows_text.is_empty() {
        parts.push(format!("Unfinished rows (done rows are only counted):\n{}", rows_text.trim_end()));
    }
    if let Some(g) = graph.as_ref().filter(|g| !g.deps.is_empty()) {
        let feats: Vec<Feature> = BoardFile::load(inp.features).map(|b| b.board.features()).unwrap_or_default();
        let ready = crate::boarddeps::ready_rows(&feats);
        parts.push(format!("Ready to start (every dependency done; start only these): {}.", if ready.is_empty() { "none".to_string() } else { ready.iter().take(10).cloned().collect::<Vec<_>>().join(", ") }));
        if let Some(c) = g.cycle() {
            parts.push(format!("The Depends column has a cycle: {}. Fix it before relying on the order.", c.join(" -> ")));
        }
    }
    if !bugs_text.is_empty() {
        parts.push(bugs_text.trim_end().to_string());
    }
    // deploy and quality
    let st = read_state(inp.root);
    let mut deploy = Vec::new();
    if let Some(c) = inp.cluster.as_ref().filter(|_| service) {
        deploy.push(format!("cluster {c}"));
    }
    for (k, label) in [("image", "image"), ("deploy_status", "last deploy_status"), ("reachable", "reachable at")].into_iter().filter(|_| service) {
        if let Some(v) = st.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            deploy.push(format!("{label}: {}", v.lines().next().unwrap_or("")));
        }
    }
    if let Some(r) = &results {
        let pass = r.tests.values().filter(|o| **o == crate::testresults::Outcome::Pass).count();
        deploy.push(format!("last test run {pass}/{} pass ({})", r.tests.len(), crate::testresults::ago(crate::clock::now_unix(), r.ran_at)));
    }
    if let Some(c) = crate::coverage::load(inp.root).and_then(|c| c.overall()) {
        deploy.push(format!("coverage {c:.0}%"));
    }
    if !deploy.is_empty() {
        parts.push(format!("Deploy and checks: {}.", deploy.join("; ")));
    }
    // files, the last session and what was in progress
    let files = recent_files(inp.root, 8);
    if !files.is_empty() {
        parts.push(format!("Files changed recently: {}.", files.join(", ")));
    }
    if let Some(h) = inp.history {
        if let Some(s) = h.latest("summary") {
            parts.push(format!("Last session summary: {}", crate::agent::truncate(&s.text, 500).replace('\n', " ")));
        }
    }
    if let Some(a) = crate::autopilot::State::load(&inp.root.join(".iyakki/autopilot.json")) {
        if a.status != "finished" {
            parts.push(format!("Last autopilot run ({}) stopped at step {}{}; goal: {}.", a.status, a.step + 1, if a.current_row.is_empty() { String::new() } else { format!(", working on {}", a.current_row) }, crate::agent::truncate(&a.goal, 100)));
        }
    }
    parts.push("Continue from this state. Do not recreate files or rows that exist; read them first (read_file, board_list). Pick the next unfinished row.".into());
    let notes = crate::notes::context_block(notes_root, crate::notes::DEFAULT_BUDGET);
    let mut text = parts.join("\n");
    if text.len() > MAX_CHARS {
        // the instruction and the stack stay; the middle is cut
        let tail = parts.last().cloned().unwrap_or_default();
        let head: String = text.chars().take(MAX_CHARS - tail.len() - 40).collect();
        text = format!("{head}\n[… cut to fit …]\n{tail}");
    }
    if notes.is_empty() {
        return text;
    }
    // after the first line, before the stack: the notes are the first thing a model reads of the project
    match text.split_once('\n') {
        Some((first, rest)) => format!("{first}\n{notes}\n{rest}"),
        None => format!("{text}\n{notes}"),
    }
}

/// "stack: Go · board 3/10 done · 1 bug": the footer of the chat.
pub fn footer(root: &Path, features: &Path, stack_override: &str) -> String {
    let stack = detect_stack(root, stack_override);
    let mut s = format!("stack: {}", stack.label());
    if let Ok(bf) = BoardFile::load(features) {
        let all = bf.board.features();
        let feats: Vec<&Feature> = all.iter().filter(|f| !f.is_bug()).collect();
        s.push_str(&format!(" · board {}/{} done", feats.iter().filter(|f| f.status == "done").count(), feats.len()));
        let bugs = all.iter().filter(|f| f.is_bug() && f.status != "done").count();
        if bugs > 0 {
            s.push_str(&format!(" · {bugs} bug{}", if bugs == 1 { "" } else { "s" }));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-pstate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_project_with_iyakki_app_json_pins_the_authoring_skills() {
        let d = std::env::temp_dir().join(format!("iyakki-ps-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("srv")).unwrap();
        assert_eq!(detect_stack(&d, "").pinned_skills("hello"), Vec::<&str>::new());
        std::fs::write(d.join("srv/iyakki-app.json"), r#"{"kind":"mcp","name":"x"}"#).unwrap();
        let st = detect_stack(&d, "");
        assert_eq!(st.pinned_skills("hello"), vec!["iyakki-app-author", "iyakki-mcp-author"]);
        std::fs::write(d.join("iyakki-app.json"), r#"{"kind":"skill","name":"x"}"#).unwrap();
        assert_eq!(detect_stack(&d, "").pinned_skills("hi"), vec!["iyakki-app-author", "iyakki-skill-author"]);
        // started from the New app entry in an empty workspace
        assert_eq!(Stack::default().pinned_skills("New app, MCP server or skill"), vec!["iyakki-app-author", "iyakki-mcp-author"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_stack_is_detected_from_its_files_with_its_commands_and_skill() {
        let cases: &[(&str, &[(&str, &str)], &str, &str, &str)] = &[
            ("go", &[("go.mod", "module x")], "Go", "lang-go", "go test ./..."),
            ("node", &[("package.json", "{\"name\":\"a\"}")], "JavaScript", "lang-javascript", "npm test"),
            ("ts", &[("package.json", "{}"), ("tsconfig.json", "{}")], "TypeScript", "lang-typescript", "npm test"),
            ("tsdep", &[("package.json", "{\"devDependencies\":{\"typescript\":\"5\"}}")], "TypeScript", "lang-typescript", "npm test"),
            ("py", &[("requirements.txt", "flask")], "Python", "lang-python", "python3 -m pytest"),
            ("pyproj", &[("pyproject.toml", "[project]")], "Python", "lang-python", "python3 -m pytest"),
            ("java", &[("pom.xml", "<project/>")], "Java", "lang-java", "mvn test"),
            ("gradle", &[("build.gradle", "")], "Java", "lang-java", "gradle test"),
            ("rust", &[("Cargo.toml", "[package]")], "Rust", "lang-rust", "cargo test"),
            ("cs", &[("App.csproj", "<Project/>")], "C#", "lang-csharp", "dotnet test"),
            ("php", &[("composer.json", "{}")], "PHP", "lang-php", "vendor/bin/phpunit"),
            ("c", &[("Makefile", "all:"), ("main.c", "int main(){}")], "C", "lang-c", "make test"),
            ("cpp", &[("CMakeLists.txt", ""), ("src/main.cpp", "int main(){}")], "C++", "lang-cpp", "make test"),
        ];
        for (tag, files, name, skill, test) in cases {
            let d = dir(tag);
            for (f, t) in *files {
                if let Some(p) = Path::new(f).parent() {
                    std::fs::create_dir_all(d.join(p)).unwrap();
                }
                std::fs::write(d.join(f), t).unwrap();
            }
            let st = detect_stack(&d, "");
            assert_eq!((st.names.first().map(String::as_str), st.lang_skill, st.test.as_str()), (Some(*name), Some(*skill), *test), "{tag}: {st:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
        // a project in a subfolder, two languages, a container: the first found is the main one and the pinned skills follow
        let d = dir("multi");
        std::fs::create_dir_all(d.join("api")).unwrap();
        std::fs::create_dir_all(d.join("deploy")).unwrap();
        std::fs::write(d.join("api/go.mod"), "module api").unwrap();
        std::fs::write(d.join("package.json"), "{}").unwrap();
        std::fs::write(d.join("Dockerfile"), "FROM scratch").unwrap();
        std::fs::write(d.join("deploy/app.yaml"), "kind: Deployment").unwrap();
        let st = detect_stack(&d, "");
        assert_eq!(st.names, vec!["JavaScript".to_string(), "Go".to_string()]);
        assert!(st.dockerfile && st.deploy_dir);
        assert_eq!(st.pinned_skills("hello"), vec!["lang-javascript", "k3s-dev-best-practices"]);
        // the person's override wins, in any spelling; an unknown name is ignored
        assert_eq!(detect_stack(&d, "golang").names, vec!["Go".to_string()]);
        assert_eq!(detect_stack(&d, "TS").lang_skill, Some("lang-typescript"));
        assert_eq!(detect_stack(&d, "cobol").names, vec!["JavaScript".to_string(), "Go".to_string()]);
        // no container and no deploy words: only the language; a deploy request adds the k3s practices
        let e = dir("plain");
        std::fs::write(e.join("Cargo.toml"), "").unwrap();
        let st = detect_stack(&e, "");
        assert_eq!(st.pinned_skills("add a button"), vec!["lang-rust"]);
        assert_eq!(st.pinned_skills("deploy this to k3s"), vec!["lang-rust", "k3s-dev-best-practices"]);
        assert!(detect_stack(&dir("empty"), "").names.is_empty());
        for t in ["multi", "plain", "empty"] {
            let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("iyakki-pstate-{}-{t}", std::process::id())));
        }
    }

    fn board_of(rows: usize) -> String {
        let mut t = crate::featureboard::EMPTY_BOARD.to_string();
        for i in 1..=rows {
            let st = match i % 5 { 0 => "done", 1 => "partial", 2 => "planned", 3 => "pending", _ => "blocked" };
            t.push_str(&format!("| F-APP-{i:02} | Feature number {i} with a fairly long descriptive title for the board | {st} | — | 2026-10-14 09:{:02} |  | some note about row {i} |\n", i % 60));
        }
        t
    }

    #[test]
    fn the_state_block_has_stack_board_bugs_deploy_and_the_instruction_and_stays_small_with_sixty_three_rows() {
        let d = dir("block");
        std::fs::write(d.join("go.mod"), "module x").unwrap();
        std::fs::write(d.join("main.go"), "package main").unwrap();
        let mut board = board_of(63);
        board.push_str("| B-APP-01 | web crashes at start | planned | — |  |  |  |\n");
        std::fs::write(d.join("FEATURES.md"), &board).unwrap();
        let mut bf = BoardFile::load(&d.join("FEATURES.md")).unwrap();
        bf.board.update("B-APP-01", &crate::featureboard::Update { severity: Some("high".into()), logs: Some(vec![".iyakki/bugs/B-APP-01/".into()]), ..Default::default() }).unwrap();
        bf.save().unwrap();
        note(&d, "image", "localhost/web:abc123");
        note(&d, "deploy_status", "Deployment web: 1/1 ready, Healthy");
        note(&d, "reachable", "http://192.168.1.5:30080");
        crate::testresults::save(&d, &crate::testresults::Results { ran_at: crate::clock::now_unix() - 60, tests: [("TestA", crate::testresults::Outcome::Pass), ("TestB", crate::testresults::Outcome::Fail)].map(|(k, v)| (k.to_string(), v)).into() });
        let inp = Input { name: "Shop", root: &d, features: &d.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: Some("k3s · namespace shop-dev".into()), history: None };
        let s = build(&inp);
        assert!(s.contains("Stack: Go") && s.contains("go test ./...") && s.contains("go run ."), "{s}");
        assert!(s.contains("63 rows, 12 done"), "{s}");
        assert!(s.contains("F-APP-01 [partial]") && s.contains("started 09:01") && s.contains("some note about row 1"), "{s}");
        assert!(!s.contains("F-APP-05 "), "done rows are only counted");
        assert!(s.contains("more unfinished rows") && s.contains("Open bugs (1)") && s.contains("B-APP-01 [planned]") && s.contains("(high)") && s.contains(".iyakki/bugs/B-APP-01/"), "{s}");
        assert!(s.contains("cluster k3s · namespace shop-dev") && s.contains("image: localhost/web:abc123") && s.contains("reachable at: http://192.168.1.5:30080") && s.contains("last test run 1/2 pass"), "{s}");
        assert!(s.contains("main.go"), "recently changed files: {s}");
        assert!(s.ends_with("Pick the next unfinished row."), "{s}");
        assert!(s.len() <= MAX_CHARS + 60, "about 900 tokens at most: {} chars", s.len());
        // it comes from the disk each time: a changed file shows at once, and a missing board is said
        std::fs::write(d.join("FEATURES.md"), board_of(3)).unwrap();
        let s2 = build(&inp);
        assert!(s2.contains("3 rows, 0 done") && !s2.contains("F-APP-63"), "{s2}");
        std::fs::remove_file(d.join("FEATURES.md")).unwrap();
        assert!(build(&inp).contains("Feature board: none yet"));
        // the footer
        std::fs::write(d.join("FEATURES.md"), &board).unwrap();
        assert_eq!(footer(&d, &d.join("FEATURES.md"), ""), "stack: Go · board 12/63 done · 1 bug");
        assert_eq!(footer(&d, &d.join("FEATURES.md"), "rust"), "stack: Rust · board 12/63 done · 1 bug", "the override is shown");
        let _ = std::fs::remove_dir_all(&d);
    }
}
