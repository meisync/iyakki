//! A deterministic status tracker for the feature board of an AI IDE workspace.
//!
//! Small models plan work on the board (`board_add`) and then never move the rows. This tracker watches the tool calls of
//! the workspace chat and moves the rows **the assistant itself added** (never a row a person wrote) from what the tools
//! really did:
//!
//! * work begins (`write_file`, `deploy_build`, `deploy_load`, `k8s_apply`): a matching `planned` row becomes `partial`;
//! * a tool succeeds: the matching row becomes `done` once every step it names has succeeded (a row "build and deploy" needs
//!   both `deploy_build` and `k8s_apply`); verifying rows need a ready rollout (`deploy_status` / `k8s_status` with every
//!   workload ready) or a passing test command;
//! * a tool fails: the row stays `partial` and gets a short `auto:` note with the reason;
//! * model prose never moves a row, `done`/`blocked` rows are never touched, and nothing is ever moved backwards.
//!
//! Rows are matched by words in their title (see [`kinds_of`]) or, for rows that start with write/create/add, by the file
//! name or kind of file written. The state lives in `.iyakki/board-auto.json` so it survives a restart of the chat.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::featureboard::{BoardFile, Feature, Update};

/// A step the tools can prove.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Build,
    Load,
    Apply,
    Verify,
    /// The workspace's namespace exists (k8s_status says so).
    Namespace,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Load => "load",
            Kind::Apply => "apply",
            Kind::Verify => "verify",
            Kind::Namespace => "namespace",
        }
    }
    fn words(self) -> &'static [&'static str] {
        match self {
            Kind::Build => &["build"],
            Kind::Load => &["load", "push", "registry", "import"],
            Kind::Apply => &["apply", "manifest", "deploy", "service", "kubectl", "expose"],
            Kind::Namespace => &["namespace"],
            Kind::Verify => &["verify", "test", "check", "status", "running", "rollout", "health", "reachable", "curl", "confirm", "access"],
        }
    }
}

const ALL_KINDS: [Kind; 5] = [Kind::Build, Kind::Load, Kind::Apply, Kind::Verify, Kind::Namespace];
const AUTHORING: [&str; 9] = ["write", "create", "add", "generate", "implement", "make", "draft", "scaffold", "author"];

fn has_word(text: &str, w: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|t| t == w || t.strip_suffix('s') == Some(w) || (w.len() >= 5 && t.starts_with(w)))
}

/// The steps a row's title names (empty for a row about something the tools cannot prove).
pub fn kinds_of(title: &str) -> BTreeSet<Kind> {
    let l = title.to_lowercase();
    ALL_KINDS.into_iter().filter(|k| k.words().iter().any(|w| has_word(&l, w))).collect()
}

/// A row that asks for a file to be written (matched by `write_file`, not by deploy steps).
pub fn is_authoring(title: &str) -> bool {
    let l = title.to_lowercase();
    let first = l.split(|c: char| !c.is_alphanumeric()).find(|t| !t.is_empty()).unwrap_or("");
    AUTHORING.contains(&first)
}

/// Does a file written match an authoring row: by its name, or by what kind of file it is.
fn file_matches(title: &str, path: &str) -> bool {
    let l = title.to_lowercase();
    let base = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if base.is_empty() {
        return false;
    }
    if l.contains(&base) {
        return true;
    }
    let ext = base.rsplit_once('.').map_or("", |x| x.1);
    let words: &[&str] = if base == "dockerfile" || base == "containerfile" {
        &["dockerfile", "containerfile", "docker file"]
    } else if matches!(ext, "yaml" | "yml") {
        &["manifest", "yaml", "deployment", "service", "k8s", "kubernetes"]
    } else if matches!(ext, "js" | "ts" | "py" | "go" | "rs" | "rb" | "java" | "php" | "mjs") {
        &["server", "app", "code", "script", "index", "api", "route", "endpoint", "main", "program"]
    } else if matches!(ext, "json") {
        &["package", "config"]
    } else if matches!(ext, "html" | "css") {
        &["page", "html", "css", "front", "ui"]
    } else {
        &[]
    };
    words.iter().any(|w| l.contains(w))
}

/// Do these tool results show a healthy, fully ready deployment?
pub fn all_ready(text: &str) -> bool {
    let l = text.to_lowercase();
    if ["imagepullbackoff", "errimagepull", "crashloop", "pending", "unhealthy", "does not exist yet", "nothing is deployed", "error", "timed out"].iter().any(|b| l.contains(b)) {
        return false;
    }
    let mut seen = false;
    for t in l.split_whitespace() {
        if let Some((a, b)) = t.split_once('/') {
            if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.trim_end_matches(',').parse::<u32>()) {
                if b == 0 || a < b {
                    return false;
                }
                seen = true;
            }
        }
    }
    seen || l.contains(": complete")
}

/// What one tool call proves, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Write(String),
    Step(Kind),
    /// A passing test command (matches verifying rows that say "test").
    Test,
}

pub fn events_of(name: &str, args: &Value) -> Vec<Event> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match name {
        "write_file" => vec![Event::Write(s("path"))],
        "deploy_build" => vec![Event::Step(Kind::Build)],
        "deploy_load" => vec![Event::Step(Kind::Load)],
        "k8s_apply" => vec![Event::Step(Kind::Apply)],
        "deploy_status" => vec![Event::Step(Kind::Verify), Event::Step(Kind::Namespace)],
        "k8s_status" => vec![Event::Step(Kind::Verify), Event::Step(Kind::Namespace)],
        // the teardown removes the namespace: its success is the proof that the namespace is gone
        "k8s_teardown" => vec![Event::Step(Kind::Namespace)],
        "run_command" => {
            let prog = s("program");
            let a: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            if matches!(prog.as_str(), "npm" | "node" | "pytest" | "cargo" | "go" | "python" | "python3" | "make" | "yarn" | "pnpm") && a.iter().any(|x| x.contains("test")) { vec![Event::Test] } else { vec![] }
        }
        _ => vec![],
    }
}

fn matches_row(ev: &Event, f: &Feature) -> bool {
    let title = &f.title;
    match ev {
        Event::Write(p) => is_authoring(title) && file_matches(title, p),
        // a plan row such as "Create namespace x" starts with a verb but is proved by the namespace existing
        Event::Step(k) => (!is_authoring(title) || *k == Kind::Namespace) && kinds_of(title).contains(k),
        Event::Test => !is_authoring(title) && has_word(&title.to_lowercase(), "test"),
    }
}

fn kind_of_event(ev: &Event) -> Option<Kind> {
    match ev {
        Event::Step(k) => Some(*k),
        Event::Test => Some(Kind::Verify),
        Event::Write(_) => None,
    }
}

pub type Audit = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone)]
pub struct Tracker {
    features: PathBuf,
    state: PathBuf,
    audit: Option<Audit>,
}

impl Tracker {
    pub fn new(root: &Path, features: &Path) -> Tracker {
        Tracker { features: features.into(), state: root.join(".iyakki/board-auto.json"), audit: None }
    }

    pub fn audit_fn(&self) -> Option<Audit> {
        self.audit.clone()
    }

    pub fn with_audit(mut self, a: Audit) -> Tracker {
        self.audit = Some(a);
        self
    }

    fn read_state(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let v: Value = std::fs::read_to_string(&self.state).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let set = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        (set("ids"), set("seen"))
    }

    fn write_state(&self, ids: &BTreeSet<String>, seen: &BTreeSet<String>) {
        if let Some(d) = self.state.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&self.state, serde_json::json!({"ids": ids, "seen": seen, "verified": self.verified_ids()}).to_string());
    }

    /// The rows the tracker has seen proved by a tool (a successful, verified step): the only rows `board_update` may set to
    /// done, and the only ones the autopilot summary counts as done.
    pub fn verified_ids(&self) -> BTreeSet<String> {
        let v: Value = std::fs::read_to_string(&self.state).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        v.get("verified").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    }

    pub fn is_verified(&self, id: &str) -> bool {
        self.verified_ids().contains(id)
    }

    pub fn mark_verified(&self, new: &[String]) {
        let (ids, seen) = self.read_state();
        let mut all = self.verified_ids();
        all.extend(new.iter().cloned());
        if let Some(d) = self.state.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&self.state, serde_json::json!({"ids": ids, "seen": seen, "verified": all}).to_string());
    }

    /// The assistant added this row: from now on the tracker may move it.
    pub fn own(&self, id: &str) {
        let (mut ids, seen) = self.read_state();
        ids.insert(id.to_string());
        self.write_state(&ids, &seen);
    }

    /// The ids of the rows the assistant added in this workspace.
    pub fn owned_ids(&self) -> BTreeSet<String> {
        self.read_state().0
    }

    /// The rows the assistant added that are not done yet, as "F-APP-03 (planned)".
    pub fn unresolved(&self) -> Vec<String> {
        let (ids, _) = self.read_state();
        let Ok(bf) = BoardFile::load(&self.features) else { return vec![] };
        bf.board.features().into_iter().filter(|f| !f.is_bug() && ids.contains(&f.id) && matches!(f.status.as_str(), "planned" | "partial")).map(|f| format!("{} ({})", f.id, f.status)).collect()
    }

    /// A call is about to run: matching planned rows start.
    pub fn before(&self, name: &str, args: &Value) {
        for ev in events_of(name, args) {
            if matches!(ev, Event::Test | Event::Step(Kind::Verify) | Event::Step(Kind::Namespace)) {
                continue;
            }
            self.apply(&ev, name, Outcome::Started);
        }
    }

    /// A call finished.
    pub fn after(&self, name: &str, args: &Value, result: &Result<String, String>) {
        for ev in events_of(name, args) {
            let outcome = match result {
                Ok(text) => {
                    // a status check proves nothing unless everything is ready; the namespace only when it exists
                    if matches!(ev, Event::Step(Kind::Verify)) && !all_ready(text) {
                        continue;
                    }
                    if matches!(ev, Event::Step(Kind::Namespace)) && name != "k8s_teardown" && !text.contains("(exists)") {
                        continue;
                    }
                    Outcome::Succeeded
                }
                Err(e) => {
                    if matches!(ev, Event::Step(Kind::Verify) | Event::Step(Kind::Namespace)) {
                        continue;
                    }
                    Outcome::Failed(e.lines().next().unwrap_or("").chars().take(90).collect())
                }
            };
            self.apply(&ev, name, outcome);
        }
    }

    fn apply(&self, ev: &Event, tool: &str, outcome: Outcome) {
        let (ids, mut seen) = self.read_state();
        if ids.is_empty() {
            return;
        }
        let Ok(mut bf) = BoardFile::load(&self.features) else { return };
        if let (Outcome::Succeeded, Some(k)) = (&outcome, kind_of_event(ev)) {
            seen.insert(k.label().to_string());
        }
        let now = crate::clock::now_unix();
        let mut changed = Vec::new();
        let mut verified: Vec<String> = Vec::new();
        for f in bf.board.features() {
            // a row that was blocked as unnecessary can still be proved by the namespace existing, and only by that
            let open = matches!(f.status.as_str(), "planned" | "partial") || (f.status == "blocked" && *ev == Event::Step(Kind::Namespace) && matches!(outcome, Outcome::Succeeded));
            // bugs (B- rows) are fixed, not built: no tool success marks one
            if f.is_bug() || !ids.contains(&f.id) || !open || !matches_row(ev, &f) {
                continue;
            }
            let base = f.notes.split(" · auto:").next().unwrap_or("").trim().to_string();
            let with = |note: &str| if base.is_empty() { format!("auto: {note}") } else { format!("{base} · auto: {note}") };
            let u = match &outcome {
                Outcome::Started if f.status == "planned" => Update { status: Some("partial".into()), comment: Some(if base.is_empty() { format!("auto: {tool} started") } else { f.notes.clone() }), at: Some(now), ..Default::default() },
                Outcome::Started => continue,
                Outcome::Failed(why) => Update { status: (f.status == "planned").then(|| "partial".to_string()), comment: Some(with(&format!("{tool} failed: {why}"))), at: Some(now), ..Default::default() },
                Outcome::Succeeded => {
                    let steps = kinds_of(&f.title);
                    let complete = match ev {
                        Event::Write(_) => true,
                        _ => steps.iter().all(|k| seen.contains(k.label())),
                    };
                    if complete {
                        verified.push(f.id.clone());
                        Update { status: Some("done".into()), comment: Some(if base.is_empty() { format!("auto: {tool} succeeded") } else { base.clone() }), at: Some(now), ..Default::default() }
                    } else if f.status == "planned" {
                        Update { status: Some("partial".into()), comment: Some(with(&format!("{tool} succeeded, more steps remain"))), at: Some(now), ..Default::default() }
                    } else {
                        continue;
                    }
                }
            };
            let to = u.status.clone().unwrap_or_else(|| f.status.clone());
            if bf.board.update(&f.id, &u).is_ok() {
                changed.push(format!("{} {} -> {} ({tool})", f.id, f.status, to));
            } else {
                verified.retain(|v| *v != f.id);
            }
        }
        if !changed.is_empty() && bf.save().is_ok() {
            self.write_state(&ids, &seen);
            self.mark_verified(&verified);
            if let Some(a) = &self.audit {
                a("board.auto", &changed.join("; "));
            }
        } else if matches!(outcome, Outcome::Succeeded) {
            self.write_state(&ids, &seen);
        }
    }
}

enum Outcome {
    Started,
    Succeeded,
    Failed(String),
}

/// A row id other than `own` mentioned in the notes (for example "needs F-APP-03 first"): what a stuck row depends on.
pub fn dependency_in(notes: &str, own: &str) -> Option<String> {
    notes.split(|c: char| !(c.is_alphanumeric() || c == '-')).find(|t| t.starts_with("F-") && t.len() > 3 && t.chars().last().is_some_and(|c| c.is_ascii_digit()) && *t != own).map(str::to_string)
}

/// What kind of work a row is, for choosing its check. Only deployment rows are checked with `deploy_status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Deploy,
    Test,
    Docs,
    Config,
    Code,
    Other,
}

pub fn classify(title: &str, notes: &str) -> RowKind {
    let l = format!("{title} {notes}").to_lowercase();
    let k = kinds_of(&title.to_lowercase());
    let has = |ws: &[&str]| ws.iter().any(|w| has_word(&l, w));
    // "check that the app responds" and "verify the deployment" are deployment checks; "test" rows are tests
    if k.contains(&Kind::Apply) || (k.contains(&Kind::Verify) && !has(&["test"])) || k.contains(&Kind::Namespace) || has(&["k8s", "kubernetes", "cluster", "helm", "rollout", "ingress", "kubectl", "pod"]) {
        return RowKind::Deploy;
    }
    if has(&["test"]) {
        return RowKind::Test;
    }
    // a row that names source files is a code row
    if files_named(&l).iter().any(|f| !f.ends_with(".md") && !f.ends_with(".txt")) {
        return RowKind::Code;
    }
    if has(&["readme", "docs", "documentation", "comment", "changelog"]) {
        return RowKind::Docs;
    }
    if has(&["config", "configure", "setting", "settings", "env"]) {
        return RowKind::Config;
    }
    if is_authoring(title) || has(&["create", "build", "implement", "add", "write", "make", "player", "viewer", "page", "app", "ui", "frontend", "backend", "api", "server", "component", "feature", "script", "module", "function", "client", "widget", "screen", "form"]) {
        return RowKind::Code;
    }
    RowKind::Other
}

/// File names mentioned in a text (`server.js`, `public/index.html`).
pub fn files_named(text: &str) -> Vec<String> {
    let exts = ["js", "mjs", "cjs", "ts", "py", "json", "yaml", "yml", "html", "css", "rs", "go", "md", "sh", "txt"];
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"' | '\'' | '`')) {
        let w = w.trim_matches(|c: char| matches!(c, '.' | ':'));
        if let Some((stem, ext)) = w.rsplit_once('.') {
            if !stem.is_empty() && exts.contains(&ext) && stem.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.')) && !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
    }
    out
}

/// A text with its digits removed and spaces collapsed: counters, timestamps and attempt numbers do not make two results differ.
pub fn normalise(text: &str) -> String {
    text.chars().filter(|c| !c.is_ascii_digit()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}
