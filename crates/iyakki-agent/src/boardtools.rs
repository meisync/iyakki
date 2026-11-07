//! The assistant's tools inside an AI IDE workspace. [`ProjectTools`] wraps the person's ordinary tools and
//!
//! * keeps every file path (and absolute path argument of `run_command`) inside the workspace root, refusing `..`,
//!   absolute paths elsewhere and symlinks that leave it, with a message that says so;
//! * removes the tools that run an arbitrary shell line (`terminal`), which cannot be kept inside a folder;
//! * labels every approval with the workspace's name;
//! * adds `board_list`, `board_update` and `board_add` for the workspace's features file. Writes ask first and show the
//!   row as it will be.
//!
//! The wrapper is only built for a workspace's chat, so the board tools are offered nowhere else.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::featureboard::{Board, BoardFile, NewFeature, Update};

pub const BOARD_TOOLS: [&str; 4] = ["board_list", "board_update", "board_add", "board_depends"];
/// Argument names that hold a path.
const PATH_KEYS: [&str; 12] = ["path", "file", "dir", "directory", "cwd", "src", "dst", "from", "to", "source", "dest", "target"];

pub struct ProjectTools {
    inner: Arc<dyn Tools>,
    root: PathBuf,
    name: String,
    features: PathBuf,
    /// The workspace's connected cluster (only when its profile answered): the k8s_* and deploy_* tools.
    k8s: Option<Arc<crate::k8stools::K8sTools>>,
    /// Platform services (Gitea, a private store): the `platform_*` tools, offered with the cluster tools.
    platform: Option<Arc<crate::platform::tools::PlatformTools>>,
    track: crate::boardtrack::Tracker,
    /// The project's test command (program, arguments), when the settings name one: a claim of done on a row about tests runs it.
    test_cmd: Option<(String, Vec<String>)>,
    /// The workspace's history (the knowledge base) for `history_search`.
    history: Option<crate::history::History>,
    /// Failed harness verifications per row, and the last refused claim (an identical one twice changes the state instead).
    verify_fails: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    /// Hard limits of the current autopilot step (see [`Tools::begin_step`]).
    step: std::sync::Mutex<StepState>,
    /// Bumped when something changed in the world (a file written, a command run, a row verified): a parked row may be retried then.
    world: std::sync::atomic::AtomicU64,
    parked: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// Files written in this run, for code rows that name none.
    written: std::sync::Mutex<Vec<String>>,
    /// The last failed build, test or check (tool, output), kept as evidence for a bug report when no pod exists.
    last_failure: std::sync::Mutex<Option<(String, String)>>,
    /// The diagnosis of the last failed check, for the next Autopilot step.
    diag: std::sync::Mutex<Option<(String, String)>>,
    /// Per row: the world version at its last failed check, the fix attempts that changed something since, and the stalls that did not.
    fixes: std::sync::Mutex<std::collections::HashMap<String, (Option<u64>, usize, usize)>>,
    /// Who writes the notes this chat makes (`assistant`, `Agent 2`, `Claude Code`) and where the project's notes live (the real workspace, also for a task's copy).
    author: String,
    notes_root: PathBuf,
    /// The agent this chat is in the workspace's agent registry (`assign`): its steps, files and notes are reported there.
    agent_key: Option<String>,
}

/// Where a workspace names its test command (told to the model and the person when a test row has no check).
const NO_TEST_CMD: &str = "set \"commands\": {\"test\": \"cargo test\"} in .iyakki/project.json";

#[derive(Default)]
struct StepState {
    board_updates: usize,
    end: Option<String>,
}

impl ProjectTools {
    pub fn new(inner: Arc<dyn Tools>, root: &Path, name: &str, features: &Path) -> ProjectTools {
        ProjectTools { inner, root: root.into(), name: name.into(), features: features.into(), k8s: None, platform: None, track: crate::boardtrack::Tracker::new(root, features), test_cmd: None, history: None, verify_fails: Default::default(), step: Default::default(), world: Default::default(), parked: Default::default(), written: Default::default(), diag: Default::default(), fixes: Default::default(), last_failure: Default::default(), author: "assistant".into(), notes_root: root.into(), agent_key: None }
    }

    /// The name notes written through this chat carry, and the workspace folder that holds the notes (a task works in a copy; its notes belong to the workspace).
    pub fn with_notes(mut self, author: &str, notes_root: &Path) -> ProjectTools {
        self.author = author.into();
        self.notes_root = notes_root.into();
        self
    }

    /// Offer `history_search` over the workspace history.
    pub fn with_history(mut self, h: crate::history::History) -> ProjectTools {
        self.history = Some(h);
        self
    }

    /// The command a claim of done on a "tests" row runs to verify it (for example `npm test`).
    pub fn with_test_command(mut self, program: &str, args: &[&str]) -> ProjectTools {
        self.test_cmd = Some((program.into(), args.iter().map(|a| a.to_string()).collect()));
        self
    }

    /// The test command the workspace names (`commands.test` in `.iyakki/project.json`, program and arguments split at spaces, no shell),
    /// looked for in each folder given in turn (a task's copy first, then the workspace). None named: no test check exists, and the
    /// refusal and the review note say so.
    pub fn with_project_test_command(self, dirs: &[&Path]) -> ProjectTools {
        let cmd = dirs.iter().find_map(|d| crate::workspace::ProjectConfig::load(d).ok().map(|c| c.commands.test).filter(|t| !t.trim().is_empty()));
        match cmd {
            Some(c) => {
                let parts: Vec<&str> = c.split_whitespace().collect();
                self.with_test_command(parts[0], &parts[1..])
            }
            None => self,
        }
    }

    /// Where the tracker reports the rows it moves itself (`board.auto`).
    pub fn with_audit(mut self, a: crate::boardtrack::Audit) -> ProjectTools {
        self.track = self.track.clone().with_audit(a);
        self
    }

    /// Offer the cluster tools, and refuse anything that would start, stop or install a cluster.
    pub fn with_k8s(mut self, k: Arc<crate::k8stools::K8sTools>) -> ProjectTools {
        self.k8s = Some(k);
        self
    }


    /// Report this chat's work to the agent registry of the workspace at `notes_root` under `key` (see [`crate::assign`]): the timeline, the
    /// files it touches (a second agent on the same file is flagged; one the person queued waits) and the notes it adds.
    pub fn with_agent(mut self, key: &str) -> ProjectTools {
        self.agent_key = Some(key.into());
        self
    }

    /// Before a file write: an agent the person queued behind another one waits.
    fn queued_behind(&self, name: &str, args: &Value) -> Option<String> {
        let key = self.agent_key.as_ref()?;
        if name != "write_file" {
            return None;
        }
        let p = args.get("path").and_then(Value::as_str)?;
        let holder = crate::assign::load(&self.notes_root).must_wait(key, p)?;
        Some(format!("{p} is being changed by {holder}, and you were asked to wait for it. Do other work and come back when {holder} is done; do not write this file now."))
    }

    /// After a call: the timeline line, the touched file, the added note.
    fn report(&self, name: &str, args: &Value, r: &Result<String, String>) {
        let Some(key) = &self.agent_key else { return };
        if matches!(name, "read_file" | "list_dir" | "board_list" | "board_depends" | "note_list" | "note_get") {
            return;
        }
        let (kind, text) = crate::assign::classify(name, r.is_ok(), args, r.as_ref().map(String::as_str).unwrap_or_else(|e| e.as_str()));
        let now = crate::clock::now_unix();
        let (path, note) = (args.get("path").and_then(Value::as_str).map(str::to_string), r.as_ref().ok().and_then(|t| t.strip_prefix("added N-")).map(|t| format!("N-{}", t.split(|c: char| !c.is_ascii_digit()).next().unwrap_or(""))));
        let title = args.get("title").and_then(Value::as_str).unwrap_or("").to_string();
        crate::assign::update(&self.notes_root, |d| {
            d.step(key, kind, &text, now);
            if let (true, Some(p)) = (name == "write_file" && r.is_ok(), path) {
                d.touch_file(key, &p, 0, 0, now);
            }
            if let (true, Some(id)) = (name == "note_add", note) {
                d.note_added(key, &id, &title, now);
            }
        });
    }

    /// Offer the platform-service tools (`platform_list`, `platform_deploy`, ...).
    pub fn with_platform(mut self, p: Arc<crate::platform::tools::PlatformTools>) -> ProjectTools {
        self.platform = Some(p);
        self
    }

    /// `kubectl` / `helm` run through run_command talk to this workspace's cluster, never to `$KUBECONFIG`'s.
    fn pinned(&self, name: &str, args: &Value) -> Value {
        let (Some(k), "run_command") = (&self.k8s, name) else { return args.clone() };
        let prog = args.get("program").and_then(Value::as_str).unwrap_or("");
        let a: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        match crate::devcluster::pin_cluster_args(prog, &a, &k.profile.kubeconfig, &k.profile.context) {
            Some(p) => {
                let mut v = args.clone();
                v["args"] = json!(p);
                v
            }
            None => args.clone(),
        }
    }

    /// IYAKKI creates the workspace namespace on the first apply, so a plan row such as "Create namespace x-dev" is done once an apply
    /// worked (also when the model had marked it blocked because it could not apply a Namespace manifest).
    fn namespace_rows_done(&self) {
        let (Some(k), true) = (&self.k8s, self.features.exists()) else { return };
        let Ok(mut bf) = BoardFile::load(&self.features) else { return };
        let ids: Vec<String> = bf.board.features().iter().filter(|f| f.status != "done" && f.status != "deprecated").filter(|f| {
            let t = f.title.to_lowercase();
            t.contains("namespace") && (t.contains(&k.ns.to_lowercase()) || !t.contains("-dev") && !t.contains("kube"))
        }).map(|f| f.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            let u = Update { status: Some("done".into()), progress: Some(100), comment: Some(format!("the namespace {} is created automatically on the first apply and exists now", k.ns)), ..Default::default() };
            let _ = bf.board.update(id, &u);
        }
        let _ = bf.save();
    }

    fn refuse(&self, what: &str) -> String {
        format!("refused: {what} is outside the workspace \"{}\" ({}). In this chat the tools only reach files inside the workspace folder; ask the person to open the other folder as its own workspace.", self.name, self.root.display())
    }

    /// Check every path in a call; the first one that leaves the workspace is an error.
    pub fn guard(&self, name: &str, args: &Value) -> Result<(), String> {
        let inside = |s: &str| -> Result<(), String> {
            if s.trim().is_empty() {
                return Ok(());
            }
            crate::workspace::resolve_in(&self.root, s).map(|_| ()).map_err(|_| self.refuse(s))
        };
        if let Some(o) = args.as_object() {
            for k in PATH_KEYS {
                if let Some(Value::String(s)) = o.get(k) {
                    // urls and similar are not paths
                    if !s.contains("://") {
                        inside(s)?;
                    }
                }
            }
            if let Some(Value::Array(a)) = o.get("paths") {
                for s in a.iter().filter_map(Value::as_str) {
                    inside(s)?;
                }
            }
        }
        if name == "run_command" {
            if let Some(a) = args.get("args").and_then(Value::as_array) {
                for s in a.iter().filter_map(Value::as_str) {
                    let candidate = s.rsplit_once('=').map_or(s, |x| x.1);
                    if candidate.starts_with('/') || candidate.starts_with('~') || candidate.split('/').any(|p| p == "..") {
                        if candidate.starts_with('~') {
                            return Err(self.refuse(s));
                        }
                        inside(candidate)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn load(&self) -> Result<BoardFile, String> {
        BoardFile::load(&self.features).map_err(|e| format!("no features board in this workspace yet ({e}). Call board_add with a prefix and a title: it creates {} with the first row.", self.features.display()))
    }

    /// For adding: the board, or a new empty one when the workspace has none yet (it is created by the save).
    fn load_or_new(&self) -> Result<BoardFile, String> {
        if self.features.exists() { BoardFile::load(&self.features).map_err(|e| format!("cannot read the board: {e}")) } else { Ok(BoardFile::new_empty(&self.features)) }
    }

    fn fmt_row(f: &crate::featureboard::Feature) -> String {
        let bl = f.blockers();
        format!("{} | {} | {} | {} | {} | {}{}{}", f.id, f.status, if f.phase.is_empty() { "-" } else { &f.phase }, if f.priority.is_empty() { "-" } else { &f.priority }, f.progress.map_or("-".to_string(), |p| format!("{p}%")), f.title, if f.notes.is_empty() { String::new() } else { format!(" — notes: {}", f.notes) }, if bl.is_empty() { String::new() } else { format!(" — blockers: {}", bl.join("; ")) })
    }

    fn update_of(args: &Value) -> Update {
        // a list argument: an array of strings or one string (entries separated by commas, semicolons or lines)
        let list = |k: &str| -> Option<Vec<String>> {
            match args.get(k)? {
                Value::Array(a) => Some(a.iter().filter_map(Value::as_str).map(|x| x.trim().trim_matches('`').to_string()).filter(|x| !x.is_empty()).collect()),
                Value::String(s) => Some(crate::featureboard::split_list(s)),
                _ => None,
            }
        };
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
        Update {
            status: s("status").map(|x| x.to_lowercase()),
            phase: s("phase"),
            priority: s("priority"),
            progress: args.get("progress").and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|x| x.trim_end_matches('%').parse().ok()))).map(|p| p.min(100) as u32),
            title: s("title"),
            comment: s("comment"),
            blocker: s("blocker"),
            tests: list("tests").map(|v| v.into_iter().filter(|x| crate::featureboard::is_test_name(x)).collect()),
            acceptance: {
                // free-text entries given as tests are acceptance criteria
                let mut a: Vec<String> = list("acceptance").unwrap_or_default();
                a.extend(list("tests").unwrap_or_default().into_iter().filter(|x| !crate::featureboard::is_test_name(x)));
                (!a.is_empty()).then_some(a)
            },
            at: Some(crate::clock::now_unix()),
            depends: s("depends_on").or_else(|| list("depends").map(|v| crate::boarddeps::join_ids(&v)).filter(|x| !x.is_empty())),
            files: list("files"),
            severity: s("severity").map(|x| x.to_lowercase()),
            repro: s("repro"),
            logs: list("logs"),
            found: s("found"),
            fix: s("fix"),
        }
    }

    fn list(&self, args: &Value) -> Result<String, String> {
        let bf = self.load()?;
        let filter = args.get("filter").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let fs = bf.board.features();
        let hit = |f: &crate::featureboard::Feature| -> bool {
            if filter.is_empty() {
                return true;
            }
            if let Some(a) = filter.strip_prefix("area:") {
                return f.area().to_lowercase() == a.trim();
            }
            if let Some(p) = filter.strip_prefix("phase:") {
                return f.phase.to_lowercase() == p.trim();
            }
            if let Some(p) = filter.strip_prefix("status:") {
                return f.status == p.trim();
            }
            if filter == "blocked" || filter == "blockers" {
                return f.status == "blocked" || !f.blockers().is_empty();
            }
            f.status == filter || f.id.to_lowercase() == filter || f.id.to_lowercase().contains(&filter) || f.title.to_lowercase().contains(&filter) || f.notes.to_lowercase().contains(&filter)
        };
        let graph = crate::boarddeps::Graph::build(&fs);
        let rows: Vec<String> = fs.iter().filter(|f| hit(f)).take(80).map(|f| format!("{}{}", Self::fmt_row(f), crate::boarddeps::state_suffix(&graph, &f.id))).collect();
        let total = fs.iter().filter(|f| hit(f)).count();
        let counts = bf.board.count_by_status().into_iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        let head = format!("{} features in {} ({counts}); id | status | phase | priority | progress | title", fs.len(), self.features.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        Ok(format!("{head}\n{}{}", rows.join("\n"), if total > rows.len() { format!("\n… {} more, narrow the filter", total - rows.len()) } else { String::new() }))
    }

    /// `board_depends`: which rows can start now, which wait for what, the critical path; with `id`, that row's chain.
    fn depends_report(&self, args: &Value) -> Result<String, String> {
        let bf = self.load()?;
        let fs = bf.board.features();
        let g = crate::boarddeps::Graph::build(&fs);
        let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !id.is_empty() {
            let f = bf.board.get(&id).ok_or_else(|| format!("no feature {id}; use board_list to see the ids"))?;
            let st = g.state(&id).map(|s| s.label()).unwrap_or_default();
            let deps = g.deps.get(&id).map(|d| d.join(", ")).unwrap_or_else(|| "none".into());
            let un = g.dependants.get(&id).map(|d| d.join(", ")).unwrap_or_else(|| "none".into());
            return Ok(format!("{id} [{}] {}: {st}. Depends on: {deps}. Unblocks: {un}.{}", f.status, f.short_title(), g.unknown.get(&id).map(|u| format!(" Unknown ids in its Depends cell: {}.", u.join(", "))).unwrap_or_default()));
        }
        let ready = crate::boarddeps::ready_rows(&fs);
        let waiting: Vec<String> = g.nodes.iter().filter_map(|n| match g.state(&n.0) { Some(crate::boarddeps::DepState::Waiting(v)) => Some(format!("{} waits for {}", n.0, v.join(", "))), Some(crate::boarddeps::DepState::Blocked(v)) if !v.is_empty() => Some(format!("{} is blocked by {}", n.0, v.join(", "))), _ => None }).take(30).collect();
        let cp = g.critical_path();
        let mut out = format!("Ready to start now ({}): {}\n", ready.len(), if ready.is_empty() { "none".into() } else { ready.join(", ") });
        out.push_str(&format!("Waiting: {}\n", if waiting.is_empty() { "none".into() } else { waiting.join("; ") }));
        if !cp.is_empty() {
            out.push_str(&format!("Critical path: {}\n", cp.join(" -> ")));
        }
        if let Some(c) = g.cycle() {
            out.push_str(&format!("CYCLE in the Depends column: {}\n", c.join(" -> ")));
        }
        out.push_str("Start only ready rows; set dependencies with board_update `depends` (row ids).");
        Ok(out)
    }

    /// What `board_update` would do: the updated board, or the reason it cannot.
    fn planned_update(&self, args: &Value) -> Result<(BoardFile, String, String), String> {
        let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut bf = self.load()?;
        let before = bf.board.get(&id).ok_or_else(|| format!("no feature {id}; use board_list to see the ids"))?;
        let u = Self::update_of(args);
        // done needs proof the harness saw (a successful, verified tool result); the model's say-so, or a Tests text, is not enough
        if u.status.as_deref() == Some("done") && before.status != "done" && !self.track.is_verified(&id) {
            return Err(format!("refused: {id} cannot be set to done: done needs a passed test or a verified check that the harness saw (a tool result that succeeded, for a deploy a ready rollout). No verification has run for this row. {} Or set it to `partial`, `blocked`, or `pending` with `depends_on` (a row id or what it waits for) and say why in `comment`; the row is unchanged.", self.next_check(&before.title)));
        }
        if (Update { at: None, ..u.clone() }) == Update::default() {
            return Err("nothing to change: give status, phase, priority, progress, comment or blocker".into());
        }
        if let Some(st) = &u.status {
            if st == "pending" && u.depends.is_none() && u.comment.is_none() && before.depends().is_empty() {
                return Err(format!("marking {id} pending needs `depends_on` (a row id or what it waits for) or a `comment`"));
            }
            if matches!(st.as_str(), "partial" | "blocked") && u.comment.is_none() && u.blocker.is_none() && before.notes.is_empty() {
                return Err(format!("marking {id} {st} needs a comment saying why: give `comment` (or `blocker`)"));
            }
        }
        if let Some(d) = &u.depends {
            crate::boarddeps::validate(&bf.board.features(), &id, d).map_err(|e| format!("refused: {e}; the row is unchanged"))?;
        }
        bf.board.update(&id, &u)?;
        let after = bf.board.get(&id).ok_or("row vanished")?;
        // a row is started only when what it depends on is done (the person can override: the approval says so)
        let over = args.get("override_deps").and_then(Value::as_bool).unwrap_or(false);
        let mut diff = format!("- {}\n+ {}", Self::fmt_row(&before), Self::fmt_row(&after));
        if u.status.as_deref() == Some("partial") && before.status == "planned" {
            if let Some(why) = crate::boarddeps::not_ready_reason(&bf.board.features(), &id) {
                if !over {
                    return Err(format!("refused: {why}. Work on a ready row first (board_depends lists them), or ask the person to allow starting it anyway (`override_deps`)."));
                }
                diff.push_str(&format!("\nWARNING: starting a row whose dependencies are not done ({why})"));
            }
        }
        Ok((bf, id, diff))
    }

    /// The real failing line of a build's output and the Dockerfile's first 15 lines, numbered (empty parts left out).
    fn build_failure_block(&self, args: &Value, out: &str) -> String {
        let step = out.lines().find(|l| { let l = l.to_lowercase(); l.contains("parse error") || l.contains("error") }).or_else(|| out.lines().find(|l| l.to_lowercase().contains("failed"))).unwrap_or("").trim();
        let dir = args.get("dir").and_then(Value::as_str).unwrap_or(".");
        let df = args.get("dockerfile").and_then(Value::as_str).filter(|d| !d.is_empty()).unwrap_or("Dockerfile");
        let numbered = crate::workspace::resolve_in(&self.root, dir).ok().and_then(|d| std::fs::read_to_string(d.join(df)).ok()).map(|t| t.lines().take(15).enumerate().map(|(i, l)| format!("{}: {l}", i + 1)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        let mut s = format!("The build FAILED. Failing step: {step}\n");
        if !numbered.is_empty() {
            s.push_str(&format!("{df} (first 15 lines, numbered):\n{numbered}\n"));
        }
        s.push_str("Fix the line the error names with write_file, then run deploy_build again. Do not ask the person to edit anything.");
        s
    }

    #[cfg(test)]
    pub(crate) fn preview_refusal_text(&self, title: &str) -> String {
        self.next_check(title)
    }

    /// What would verify a row, in words for the model.
    fn next_check(&self, title: &str) -> String {
        let k = crate::boardtrack::kinds_of(title);
        let about_tests = title.to_lowercase().contains("test");
        let mut out = if k.contains(&crate::boardtrack::Kind::Apply) || k.contains(&crate::boardtrack::Kind::Verify) || k.contains(&crate::boardtrack::Kind::Namespace) {
            "Run deploy_status now (or k8s_status) and the harness marks the row done when everything is ready.".to_string()
        } else if about_tests && self.test_cmd.is_some() {
            "Run the project's test command with run_command now and the harness marks the row done when it passes.".to_string()
        } else {
            "Run the tool that does the step (write_file, deploy_build, deploy_load, k8s_apply) or a read-only check, and the harness marks the row done when it succeeds.".to_string()
        };
        if about_tests && self.test_cmd.is_none() {
            out.push_str(&format!(" This workspace names no test command ({NO_TEST_CMD}), so no automatic test check exists for this row."));
        }
        out
    }

    /// The check that fits the row: Ok(output) passed, Err(output) failed, None when no sensible automatic check exists (the row then
    /// goes to `review`). Never `deploy_status` for a row that is not about a deployment.
    fn verify_row(&self, f: &crate::featureboard::Feature) -> Option<Result<String, String>> {
        use crate::boardtrack::RowKind;
        let has = |n: &str| self.defs().iter().any(|d| d.spec.name == n);
        // tests the row lists whose last run all passed are a verified check
        if let Some(res) = crate::testresults::load(&self.root) {
            if crate::testresults::match_row(&f.tests, &res).all_passed() {
                return Some(Ok(format!("{} listed tests passed in the last run", f.tests.len())));
            }
        }
        // a row that names an address (and is not about code or tests) is verified by loading that address
        let names_url = !crate::idebrowser::urls_in(&format!("{} {}", f.title, f.notes)).is_empty();
        match crate::boardtrack::classify(&f.title, &f.notes) {
            RowKind::Deploy if has("deploy_status") => Some(match self.call_core("deploy_status", &json!({})) {
                // ready is not enough: the app must answer at its `reachable at:` address (status 200 and the notes' `expect:` words)
                Ok(t) if crate::boardtrack::all_ready(&t) => match self.http_check(f, Some(&t)) {
                    Some(Ok(h)) => Ok(format!("{t}\n{h}")),
                    Some(Err(e)) => Err(format!("{t}\n{e}")),
                    None => Ok(t),
                },
                Ok(t) => Err(t),
                Err(e) => Err(e),
            }),
            RowKind::Other if names_url => self.http_check(f, None),
            RowKind::Code => Some(self.verify_code(f)),
            RowKind::Test => self.test_cmd.as_ref().map(|(p, a)| self.call_core("run_command", &json!({"program": p, "args": a}))),
            _ => None,
        }
    }

    /// The http check of a deploy row (`idebrowser::row_check`): through `ide_browser_check` when the IDE browser is offered, else plain http.
    fn http_check(&self, f: &crate::featureboard::Feature, status: Option<&str>) -> Option<Result<String, String>> {
        let has = self.defs().iter().any(|d| d.spec.name == crate::idebrowser::CHECK);
        crate::idebrowser::row_check(&|n, a| self.inner.call(n, a), has, &self.root, &f.id, &f.title, &f.notes, status)
    }

    /// Code rows: the files the row names (or, when it names none, the files written in this run) exist, are not empty and
    /// parse (`node --check`, `python3 -m py_compile`, JSON); then the project's test command when one is set.
    fn verify_code(&self, f: &crate::featureboard::Feature) -> Result<String, String> {
        let named = crate::boardtrack::files_named(&format!("{} {}", f.title, f.notes));
        let files: Vec<String> = if named.is_empty() { self.written.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().take(5).cloned().collect() } else { named };
        if files.is_empty() {
            return Err("no file was written for this row yet: write the code with write_file first".into());
        }
        let has_run = self.defs().iter().any(|d| d.spec.name == "run_command");
        let mut checked = Vec::new();
        for file in &files {
            let path = crate::workspace::resolve_in(&self.root, file).map_err(|e| format!("{file}: {e}"))?;
            let text = std::fs::read_to_string(&path).map_err(|_| format!("{file} does not exist (or is not text)"))?;
            if text.trim().is_empty() {
                return Err(format!("{file} is empty"));
            }
            let ext = file.rsplit_once('.').map_or("", |x| x.1);
            match ext {
                "json" => {
                    serde_json::from_str::<Value>(&text).map_err(|e| format!("{file} is not valid JSON: {e}"))?;
                }
                "html" if !text.contains('<') => return Err(format!("{file} has no markup")),
                "js" | "mjs" | "cjs" | "py" if has_run => {
                    let (prog, args) = if ext == "py" { ("python3", vec!["-m", "py_compile", &file[..]]) } else { ("node", vec!["--check", &file[..]]) };
                    match self.call_core("run_command", &json!({"program": prog, "args": args})) {
                        Err(e) if !e.contains("not installed") && !e.contains("not available") && !e.contains("may not run") => return Err(format!("{file} does not parse: {}", e.lines().next().unwrap_or(""))),
                        _ => {}
                    }
                }
                _ => {}
            }
            checked.push(file.clone());
        }
        if let Some((p, a)) = &self.test_cmd {
            self.call_core("run_command", &json!({"program": p, "args": a})).map_err(|e| format!("the tests fail: {}", e.lines().next().unwrap_or("")))?;
        }
        Ok(format!("files exist and parse: {}", checked.join(", ")))
    }

    /// The lines of a failed check's output that say what is wrong (the Diagnosis and Next lines, pod reasons), at most 1,500 characters.
    fn diagnosis_of(out: &str) -> String {
        // an http check of the app (`idebrowser::row_check`): its whole report is the diagnosis (status, title, excerpt)
        if out.contains("CHECK FAIL") {
            return crate::agent::truncate(out, 1500);
        }
        let keys = ["diagnosis", "next:", "imagepullbackoff", "errimagepull", "createcontainerconfigerror", "crashloop", "oomkilled", "unhealthy", "pending", "error", "failed", "0/", "pod ", "runasnonroot", "does not exist", "is empty", "not valid", "no file was written", "does not parse"];
        let mut lines: Vec<&str> = out.lines().filter(|l| { let l = l.to_lowercase(); keys.iter().any(|k| l.contains(k)) }).collect();
        if lines.is_empty() {
            lines = out.lines().take(6).collect();
        }
        crate::agent::truncate(&lines.into_iter().take(14).collect::<Vec<_>>().join("\n"), 1500)
    }

    /// The harness changes a row's state itself (blocked, pending or review), with the reason in the notes.
    fn force_status(&self, id: &str, status: &str, why: &str, depends: Option<String>) {
        if let Ok(mut bf) = BoardFile::load(&self.features) {
            let u = Update { status: Some(status.into()), comment: Some(format!("harness: {why}")), at: Some(crate::clock::now_unix()), depends, ..Default::default() };
            if bf.board.update(id, &u).is_ok() {
                let _ = bf.save();
            }
        }
        if let Some(a) = self.track.audit_fn() {
            a("board.auto", &format!("{id} -> {status} (harness: {why})"));
        }
    }

    /// Park a row for good reason and end the step, so the next row starts: the model gets one concise instruction.
    fn park(&self, id: &str, status: &str, reason: &str, depends: Option<String>) -> String {
        self.force_status(id, status, reason, depends);
        let msg = format!("Row {id} is parked as {status}: {reason}. Continue with the next row.");
        self.step.lock().unwrap_or_else(|e| e.into_inner()).end = Some(msg.clone());
        msg
    }

    /// A claim of done without proof. The harness runs the check that fits the row and marks it done when it passes; a failing
    /// check is shown briefly once, and a second failure in the same step (or a third in the run) parks the row and ends the step;
    /// a row with no sensible automatic check goes to `review`. Nothing here depends on what the model says or on text equality.
    fn claim_done(&self, args: &Value) -> Option<Result<String, String>> {
        let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if args.get("status").and_then(Value::as_str).map(str::trim) != Some("done") || self.track.is_verified(&id) {
            return None;
        }
        let f = BoardFile::load(&self.features).ok()?.board.get(&id)?;
        if f.status == "done" {
            return None;
        }
        let world = self.world.load(std::sync::atomic::Ordering::SeqCst);
        // parked until something changed (a file written, a command run, another row done)
        if self.parked.lock().unwrap_or_else(|e| e.into_inner()).get(&id) == Some(&world) {
            self.step.lock().unwrap_or_else(|e| e.into_inner()).end = Some(format!("Row {id} stays parked: nothing has changed since its check failed. Continue with the next row."));
            return Some(Ok(format!("{id} stays parked until something changes. Continue with the next row.")));
        }
        match self.verify_row(&f) {
            Some(Ok(out)) => {
                self.track.mark_verified(&[id.clone()]);
                if f.is_bug() {
                    let _ = crate::bugs::write_evidence(&crate::bugs::bugs_dir(&self.root).join(&id), "passing.log", &format!("{}\n{out}", Self::now_stamp()));
                }
                self.resolve_bugs_for(&id, &out);
                self.verify_fails.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                self.fixes.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                self.world.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Some(a) = self.track.audit_fn() {
                    a("board.auto", &format!("{id} verified by the harness before done: {}", out.lines().next().unwrap_or("")));
                }
                None
            }
            Some(Err(out)) => {
                let mut diagnosis = Self::diagnosis_of(&out);
                if let Some(bug) = self.auto_bug(&id, &f.title, &out) {
                    diagnosis.push_str(&format!("\nBug {bug}: logs of every container are in .iyakki/bugs/{bug}/"));
                }
                *self.diag.lock().unwrap_or_else(|e| e.into_inner()) = Some((id.clone(), diagnosis.clone()));
                // a real fix attempt is a failed check after something changed (a file written, a command run, a build or apply); the same
                // failure with nothing changed is a stall
                let (attempts, stalls) = {
                    let mut m = self.fixes.lock().unwrap_or_else(|e| e.into_inner());
                    let e = m.entry(id.clone()).or_insert((None, 0, 0));
                    match e.0 {
                        Some(w) if w != world => e.1 += 1,
                        Some(_) => e.2 += 1,
                        None => {}
                    }
                    e.0 = Some(world);
                    (e.1, e.2)
                };
                let first: String = diagnosis.lines().find(|l| l.contains("Diagnosis") || l.contains("BackOff") || l.contains("Error")).or(diagnosis.lines().next()).unwrap_or("").chars().take(140).collect();
                if attempts >= 3 || stalls >= 3 {
                    self.parked.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), world);
                    self.fixes.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                    let dep = crate::boardtrack::dependency_in(&f.notes, &id);
                    let (status, reason) = match &dep {
                        Some(d) => ("pending", format!("the check still fails ({first}); waits for {d}")),
                        None => ("blocked", format!("the check still failed after {} fix attempts: {first}", attempts.max(stalls))),
                    };
                    *self.diag.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    return Some(Ok(self.park(&id, status, &reason, dep)));
                }
                // the step ends here: the next one starts with the full diagnosis and the instruction to fix and redeploy
                self.step.lock().unwrap_or_else(|e| e.into_inner()).end = Some(format!("The check for {id} failed. The diagnosis comes with the next step."));
                Some(Err(format!("{id} not done: the check failed: {first}. The next step brings the full diagnosis.")))
            }
            None => {
                self.verify_fails.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                let why = if matches!(crate::boardtrack::classify(&f.title, &f.notes), crate::boardtrack::RowKind::Test) && self.test_cmd.is_none() {
                    format!("work claimed; this workspace names no test command ({NO_TEST_CMD}), so no automatic check exists and it needs your OK")
                } else {
                    "work claimed; no automatic check applies, so it needs your OK".to_string()
                };
                Some(Ok(self.park(&id, "review", &why, None)))
            }
        }
    }

    /// `board_link_tests`: the listed tests added to the row's Tests column (existing ones kept, no duplicates).
    fn planned_link(&self, args: &Value) -> Result<(BoardFile, String, String), String> {
        let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut bf = self.load()?;
        let before = bf.board.get(&id).ok_or_else(|| format!("no feature {id}; use board_list to see the ids"))?;
        let new = Self::update_of(args).tests.unwrap_or_default();
        if new.is_empty() {
            return Err("give `tests`: the names of the tests to link".into());
        }
        let mut all = before.tests.clone();
        for t in new {
            if !all.contains(&t) {
                all.push(t);
            }
        }
        bf.board.update(&id, &Update { tests: Some(all), ..Default::default() })?;
        let after = bf.board.get(&id).ok_or("row vanished")?;
        let diff = format!("- {} tests\n+ {} tests: {}", before.tests.len(), after.tests.len(), after.tests.join(", "));
        Ok((bf, id, diff))
    }

    // (bugs: see below)
    /// A row whose title is the same once case, punctuation and spacing are ignored.
    fn same_title(bf: &BoardFile, title: &str) -> Option<crate::featureboard::Feature> {
        let norm = |t: &str| t.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let want = norm(title);
        (!want.is_empty()).then(|| bf.board.features().into_iter().find(|f| norm(&f.title) == want)).flatten()
    }

    // ---------------------------------------------------------------------------------------------------- bugs

    fn now_stamp() -> String {
        crate::featureboard::stamp_text(crate::clock::now_unix())
    }

    /// Capture the evidence of a failure and put it on the board as a `B-` row, or find the bug that already tracks the same failure
    /// (same signature) and add this sighting to it. `feature` is the row the failure was found while working on.
    fn capture_bug(&self, title: &str, severity: &str, repro: &str, pods: &[String], since: &str, lines: usize, feature: &str, auto: bool, failure: Option<String>, area: &str) -> Result<String, String> {
        let root = &self.root;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmp = crate::bugs::bugs_dir(root).join(format!(".tmp-{nanos}"));
        std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot write the evidence: {e}"))?;
        let mut ev = match &self.k8s {
            Some(k) => match k.capture_evidence(&tmp, pods, lines, since) {
                Ok(e) => e,
                Err(why) => {
                    let _ = crate::bugs::write_evidence(&tmp, "capture-error.txt", &why);
                    Default::default()
                }
            },
            None => Default::default(),
        };
        // the failing check or build output: given, or the last failure the tools saw (the only evidence of a local build)
        let failure = failure.or_else(|| self.last_failure.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(tool, out)| format!("{tool}:\n{out}")));
        if let Some(f) = &failure {
            if let Ok(n) = crate::bugs::write_evidence(&tmp, if f.contains("deploy_build") || f.contains("BUILD") { "build-output.log" } else { "failing-output.txt" }, f) {
                ev.files.push(n);
            }
            for r in crate::bugs::bad_reasons_in(f) {
                if !ev.reasons.contains(&r) {
                    ev.reasons.push(r);
                }
            }
        }
        let first_pod = ev.pods.first().cloned().unwrap_or_default();
        let image = ev.images.first().cloned().unwrap_or_default();
        let sig = crate::bugs::signature(&if ev.reasons.is_empty() { vec![title.to_string()] } else { ev.reasons.clone() }, &first_pod, &image);
        let mut index = crate::bugs::Index::load(root);
        if let Some((id, _, _, seen)) = index.find(&sig).cloned() {
            let _ = std::fs::remove_dir_all(&tmp);
            let n = seen + 1;
            if let Some(e) = index.items.get_mut(&sig) {
                e.3 = n;
            }
            index.save(root);
            return Ok(self.sighting(&id, n, failure.as_deref()));
        }
        let mut bf = self.load_or_new()?;
        let area = if !area.is_empty() { area.to_string() } else if !feature.is_empty() { crate::featureboard::area_of(feature) } else { "APP".to_string() };
        let id = bf.board.next_bug_id(&area);
        let dir = crate::bugs::bugs_dir(root).join(&id);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&tmp, &dir).map_err(|e| format!("cannot keep the evidence: {e}"))?;
        let found = Self::now_stamp();
        let summary = format!("# {id}: {title}\n\nSeverity: {severity}\nFound: {found}\nLinked feature: {}\nReasons: {}\nPods: {}\nImages: {}\nRepro: {}\n\nFiles:\n{}\n", if feature.is_empty() { "-" } else { feature }, ev.reasons.join(", "), ev.pods.join(", "), ev.images.join(", "), if repro.is_empty() { "-" } else { repro }, ev.files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n"));
        let _ = crate::bugs::write_evidence(&dir, "summary.md", &summary);
        let sev = if ["low", "med", "high"].contains(&severity) { severity } else { "med" };
        bf.board.add(&NewFeature { id: id.clone(), title: title.to_string(), phase: String::new(), status: "planned".into(), priority: String::new(), notes: if auto { "found by the harness".into() } else { String::new() } })?;
        bf.board.update(&id, &Update { severity: Some(sev.into()), repro: (!repro.is_empty()).then(|| repro.to_string()), logs: Some(vec![format!(".iyakki/bugs/{id}/")]), found: Some(found), fix: (!feature.is_empty()).then(|| feature.to_string()), at: Some(crate::clock::now_unix()), ..Default::default() })?;
        bf.save()?;
        self.track.own(&id);
        index.items.insert(sig, (id.clone(), feature.to_string(), auto, 1));
        index.save(root);
        if let Some(a) = self.track.audit_fn() {
            a("bug.report", &format!("{id}: {title} ({} files, reasons: {})", ev.files.len(), ev.reasons.join(", ")));
        }
        Ok(format!("{id} created: {title}. Evidence ({} files) is in .iyakki/bugs/{id}/ (logs of every container, describe, events, manifest, Dockerfile). Fix the cause, then verify and claim it done.", ev.files.len()))
    }

    /// The same failure again: one more sighting on the same bug (a file with this failure's output and a note), not a second bug.
    fn sighting(&self, id: &str, n: u32, failure: Option<&str>) -> String {
        let dir = crate::bugs::bugs_dir(&self.root).join(id);
        if let Some(f) = failure {
            let _ = crate::bugs::write_evidence(&dir, &format!("seen-{n}.txt"), &crate::agent::truncate(f, 20_000));
        }
        if let Ok(mut bf) = BoardFile::load(&self.features) {
            if let Some(row) = bf.board.get(id) {
                let base = row.notes.split(" · seen ").next().unwrap_or("").trim().to_string();
                let note = if base.is_empty() { format!("seen {n} times, last {}", Self::now_stamp()) } else { format!("{base} · seen {n} times, last {}", Self::now_stamp()) };
                if bf.board.update(id, &Update { comment: Some(note), ..Default::default() }).is_ok() {
                    let _ = bf.save();
                }
            }
        }
        format!("already tracked as {id} (seen {n} times); this sighting was added to .iyakki/bugs/{id}/. Do not report it again: fix the cause.")
    }

    /// The harness reports a failure it saw (a failed check, a pod that cannot start) once per signature, and says where the logs are.
    fn auto_bug(&self, feature: &str, title_hint: &str, failure: &str) -> Option<String> {
        let reasons = crate::bugs::bad_reasons_in(failure);
        if reasons.is_empty() {
            return None;
        }
        // a quick look in the index first: the same reasons and pod pattern already have a bug, so no kubectl is run again
        let pod = failure.split_whitespace().skip_while(|w| *w != "Pod").nth(1).unwrap_or("").trim_end_matches(':').to_string();
        let pre = crate::bugs::signature(&reasons, &pod, "");
        let prefix = &pre[..pre.rfind('|').map_or(pre.len(), |i| i + 1)];
        let mut index = crate::bugs::Index::load(&self.root);
        let known = if pod.is_empty() { None } else { index.items.iter().find(|(k, _)| k.starts_with(prefix)).map(|(k, v)| (k.clone(), v.0.clone(), v.3)) };
        if let Some((key, id, seen)) = known {
            if let Some(e) = index.items.get_mut(&key) {
                e.3 = seen + 1;
            }
            index.save(&self.root);
            self.sighting(&id, seen + 1, Some(failure));
            return Some(id);
        }
        let title = format!("{} ({})", reasons.join(", "), crate::agent::truncate(title_hint, 40));
        let made = self.capture_bug(&title, "high", "", &[], "", crate::bugs::DEFAULT_LINES, feature, true, Some(failure.to_string()), "").ok()?;
        made.split_whitespace().find(|w| w.starts_with("B-")).map(|w| w.trim_end_matches([':', '.', ',']).to_string())
    }

    /// The failure is fixed: the passing run is kept with the bug's evidence, the bug is done and has a finish time.
    fn resolve_bug(&self, bug_id: &str, passing: &str) {
        let dir = crate::bugs::bugs_dir(&self.root).join(bug_id);
        let _ = crate::bugs::write_evidence(&dir, "passing.log", &format!("{}\n{passing}", Self::now_stamp()));
        if let Ok(mut bf) = BoardFile::load(&self.features) {
            if bf.board.get(bug_id).is_some_and(|f| f.status != "done") {
                let u = Update { status: Some("done".into()), comment: Some("fixed; the passing run is in passing.log".into()), at: Some(crate::clock::now_unix()), ..Default::default() };
                if bf.board.update(bug_id, &u).is_ok() && bf.save().is_ok() {
                    self.track.mark_verified(&[bug_id.to_string()]);
                    if let Some(a) = self.track.audit_fn() {
                        a("bug.fixed", &format!("{bug_id}: the passing run was captured"));
                    }
                }
            }
        }
    }

    /// Bugs linked to a feature row (or, with an empty id, every open bug the harness created) are resolved by this passing output.
    fn resolve_bugs_for(&self, feature: &str, passing: &str) {
        let idx = crate::bugs::Index::load(&self.root);
        let ids: Vec<String> = if feature.is_empty() { idx.items.values().filter(|v| v.2).map(|v| v.0.clone()).collect() } else { idx.for_feature(feature) };
        for id in ids {
            self.resolve_bug(&id, passing);
        }
    }

    fn bug_report(&self, args: &Value) -> Result<String, String> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let title = s("title");
        if title.is_empty() {
            return Err("give a `title` for the bug".into());
        }
        let pods: Vec<String> = args.get("pods").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(crate::bugs::DEFAULT_LINES as u64).clamp(1, crate::bugs::MAX_LINES as u64) as usize;
        self.capture_bug(&title, &s("severity").to_lowercase(), &s("repro"), &pods, &s("since"), lines, &s("feature"), false, args.get("evidence").and_then(Value::as_str).map(str::to_string), &s("area"))
    }

    /// Why a cluster tool is not available in this workspace, if it is not: its target is not a service on k3s.
    fn target_refusal(&self, tool: &str) -> Option<String> {
        if crate::target::deploy_tools_offered(&self.root) {
            return None;
        }
        if crate::target::conversion_tools().contains(&tool) && crate::target::teardown_pending(&self.root).is_some() {
            return None;
        }
        Some(crate::target::guard_message(&self.root))
    }

    fn planned_add(&self, args: &Value) -> Result<(BoardFile, String, String), String> {
        let mut bf = self.load_or_new()?;
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(|x| x.trim().to_string()).unwrap_or_default();
        if !crate::target::deploy_tools_offered(&self.root) && !s("kind").eq_ignore_ascii_case("bug") && !s("id").starts_with("B-") && crate::target::is_cluster_row(&s("title"), &s("notes")) {
            return Err(format!("{}: a cluster, image, registry or deploy row does not belong on this board. Rows for this workspace: build, test (headless), install to the launcher, verify", crate::target::guard_message(&self.root)));
        }
        // a bug is a row whose id starts with B-: `kind: bug` (or a B- id) chooses it, a bug is never given an F- id
        let is_bug = s("kind").eq_ignore_ascii_case("bug") || s("id").starts_with("B-") || s("prefix").starts_with("B-");
        if s("prefix").is_empty() && s("id").is_empty() {
            return Err("give `prefix` (an area such as BROW; the next free id is chosen) or an `id`".into());
        }
        if !s("id").is_empty() && is_bug != s("id").starts_with("B-") {
            return Err("bug ids start with B- and feature ids with F-: use `kind: bug` for a bug (the id is then chosen for you)".into());
        }
        let id = if !s("id").is_empty() { s("id") } else if is_bug { bf.board.next_bug_id(&s("prefix")) } else { bf.board.next_id(&s("prefix")) };
        if let Some(old) = Self::same_title(&bf, &s("title")) {
            return Err(format!("already exists as {}", old.id));
        }
        let nf = NewFeature { id: id.clone(), title: s("title"), phase: s("phase"), status: s("status"), priority: s("priority"), notes: s("notes") };
        if matches!(nf.status.as_str(), "partial" | "blocked") && nf.notes.is_empty() {
            return Err(format!("a {} feature needs `notes` saying why", nf.status));
        }
        bf.board.add(&nf)?;
        let extra = Self::update_of(args);
        let bug_fields = is_bug || extra.severity.is_some() || extra.repro.is_some() || extra.logs.is_some() || extra.fix.is_some();
        if bug_fields || extra.tests.as_ref().is_some_and(|t| !t.is_empty()) || extra.files.as_ref().is_some_and(|f| !f.is_empty()) || extra.acceptance.is_some() {
            bf.board.update(&id, &Update {
                tests: extra.tests.filter(|t| !t.is_empty()),
                files: extra.files.filter(|f| !f.is_empty()),
                acceptance: extra.acceptance,
                severity: if is_bug { Some(extra.severity.unwrap_or_else(|| "med".into())) } else { extra.severity },
                repro: extra.repro,
                logs: extra.logs,
                found: if is_bug { Some(extra.found.unwrap_or_else(|| crate::featureboard::stamp_text(crate::clock::now_unix()))) } else { extra.found },
                fix: extra.fix,
                ..Default::default()
            })?;
        }
        if let Some(d) = extra.depends.as_ref() {
            crate::boarddeps::validate(&bf.board.features(), &id, d).map_err(|e| format!("refused: {e}"))?;
            bf.board.update(&id, &Update { depends: Some(d.clone()), ..Default::default() })?;
        }
        let row = Self::fmt_row(&bf.board.get(&id).ok_or("row missing")?);
        Ok((bf, id, format!("+ {row}")))
    }
}

impl ProjectTools {
    fn call_core(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "history_search" if self.history.is_some() => {
                let h = self.history.as_ref().expect("checked");
                let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
                let k = args.get("k").and_then(Value::as_u64).unwrap_or(4).clamp(1, 8) as usize;
                let kind = s("kind");
                let snips = h.search(&s("query"), crate::history::Scope::parse(&s("scope")), (!kind.is_empty()).then_some(kind.as_str()), k);
                Ok(if snips.is_empty() { "No matching history.".to_string() } else { crate::history::History::render(&snips, 6000) })
            }
            "bug_report" => self.bug_report(args),
            "board_link_tests" => {
                let (mut bf, id, diff) = self.planned_link(args)?;
                bf.save()?;
                Ok(format!("linked tests to {id}\n{diff}"))
            }
            "board_list" => self.list(args),
            "board_depends" => self.depends_report(args),
            n if crate::notetools::handles(n) => crate::notetools::call(&self.notes_root, &crate::notetools::author_or(&self.author), n, args),
            "board_update" => {
                // at most three board_update calls per step, whatever they say: the fourth is not run, the row is parked, the step ends
                let n = {
                    let mut s = self.step.lock().unwrap_or_else(|e| e.into_inner());
                    s.board_updates += 1;
                    s.board_updates
                };
                if n > 3 {
                    let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
                    let open = BoardFile::load(&self.features).ok().and_then(|b| b.board.get(&id)).is_some_and(|f| matches!(f.status.as_str(), "planned" | "partial"));
                    return Ok(if open { self.park(&id, "review", "the board was updated too often in one step; the work needs your OK", None) } else {
                        self.step.lock().unwrap_or_else(|e| e.into_inner()).end = Some("The board was updated too often in one step. Continue with the next row.".into());
                        "No more board updates in this step. Continue with the next row.".into()
                    });
                }
                if let Some(r) = self.claim_done(args) {
                    return r;
                }
                let (mut bf, id, diff) = self.planned_update(args)?;
                // the same change again (the model repeating itself): nothing is written and it says so
                if let Ok(cur) = std::fs::read_to_string(&self.features) {
                    let mut probe = bf.board.clone();
                    probe.sync_totals();
                    if probe.render() == cur {
                        return Ok(format!("already set: {id} already has these values; nothing was written. Do not repeat the call; move on or say what blocks you."));
                    }
                }
                bf.save()?;
                let w = bf.board.get(&id).map(|f| f.warnings()).unwrap_or_default();
                Ok(format!("updated {id}\n{diff}{}", if w.is_empty() { String::new() } else { format!("\nnote: {}", w.join("; ")) }))
            }
            "board_add" => {
                // the same title again (the model repeating its plan): no second row; tests, files and acceptance are merged into the existing one
                if let Ok(mut bf) = self.load() {
                    if let Some(old) = Self::same_title(&bf, args.get("title").and_then(Value::as_str).unwrap_or("")) {
                        let extra = Self::update_of(args);
                        let merged = |cur: &[String], new: Option<Vec<String>>| new.filter(|n| !n.is_empty()).map(|n| { let mut all = cur.to_vec(); all.extend(n.into_iter().filter(|x| !cur.contains(x))); all });
                        let u = Update { tests: merged(&old.tests, extra.tests), files: merged(&old.files, extra.files), acceptance: merged(&old.criteria, extra.acceptance), ..Default::default() };
                        if u.tests.is_some() || u.files.is_some() || u.acceptance.is_some() {
                            bf.board.update(&old.id, &u)?;
                            bf.save()?;
                        }
                        return Ok(format!("already exists as {} ({}); no second row was added{}", old.id, old.status, if u.tests.is_some() || u.files.is_some() || u.acceptance.is_some() { ", its tests, files and criteria were merged" } else { "" }));
                    }
                }
                let (mut bf, id, diff) = self.planned_add(args)?;
                bf.save()?;
                let w = bf.board.get(&id).map(|f| f.warnings()).unwrap_or_default();
                Ok(format!("added {id}\n{diff}{}", if w.is_empty() { String::new() } else { format!("\nnote: {}", w.join("; ")) }))
            }
            n if crate::k8stools::K8sTools::handles(n) && self.k8s.is_some() => {
                if let Some(why) = self.target_refusal(n) {
                    return Err(why);
                }
                let r = self.k8s.as_ref().expect("checked").call(n, args);
                if n == "k8s_teardown" && r.is_ok() {
                    crate::target::teardown_done(&self.root);
                }
                r
            }
            n if crate::platform::tools::handles(n) && self.platform.is_some() => self.platform.as_ref().expect("checked").call(n, args),
            "terminal" => Err(format!("refused: a shell line cannot be kept inside the workspace \"{}\"; use run_command with a program and arguments, or the file tools", self.name)),
            _ => {
                if let Some(why) = self.refusal(name, args) {
                    return Err(why);
                }
                self.guard(name, args)?;
                let args = self.pinned(name, args);
                self.inner.call(name, &args)
            }
        }
    }
}

impl Tools for ProjectTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v: Vec<ToolDef> = self.inner.defs().into_iter().filter(|d| d.spec.name != "terminal").collect();
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
        let file = self.features.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "FEATURES.md".into());
        let tool = |name: &str, desc: String, schema: Value, risk: Risk, untrusted: bool| ToolDef { spec: ToolSpec { name: name.into(), description: desc, schema }, risk, untrusted };
        v.push(tool("board_list", format!("List the features on this workspace's tracking board ({file}): id, status, phase, priority, progress, title, notes and blockers. `filter` is optional: a status (done, partial, planned, blocked), `area:BROW`, `phase:P5`, `blocked` (anything with a blocker), or words to look for in id, title and notes."), obj(json!({"filter": {"type": "string"}}), &[]), Risk::Safe, true));
        v.push(tool("board_update", format!("Change a feature on the board ({file}): `id` plus any of `status`, `phase`, `priority`, `progress` (0-100), `comment` (replaces the notes), `blocker` (added in the file's own convention). Moving to partial, blocked or pending needs a comment saying why; `pending` (waiting for something) takes `depends_on`: another row's id or a few words, and the run comes back to it. done is only accepted after a verified check: claiming it runs the check for you. `depends` (row ids this row waits for, e.g. [\"F-APP-01\"]: unknown ids and cycles are refused; a row whose dependencies are not done cannot be started). Only the cells you give change; the totals line is kept in sync. The person approves the change and sees the row before and after."), obj(json!({"id": {"type": "string"}, "status": {"type": "string"}, "phase": {"type": "string"}, "priority": {"type": "string"}, "progress": {"type": "integer"}, "comment": {"type": "string"}, "blocker": {"type": "string"}, "depends_on": {"type": "string"}, "depends": {"type": "array", "items": {"type": "string"}}, "override_deps": {"type": "boolean"}, "severity": {"type": "string"}, "repro": {"type": "string"}, "logs": {"type": "array", "items": {"type": "string"}}, "fix": {"type": "string"}, "tests": {"type": "array", "items": {"type": "string"}}, "acceptance": {"type": "array", "items": {"type": "string"}}, "files": {"type": "array", "items": {"type": "string"}}}), &["id"]), Risk::Ask, false));
        v.push(tool("board_depends", format!("Show how the rows of {file} depend on each other: which rows are ready to start (every dependency done), which wait for what, and the critical path. With `id`: that row's dependencies and what it unblocks. Pick the next row from the ready ones. Read-only."), obj(json!({"id": {"type": "string"}}), &[]), Risk::Safe, true));
        v.extend(crate::notetools::defs(&self.notes_root));
        if let Some(k) = &self.k8s {
            // a workspace that is not a service has no deploy tools (only the removal of the dev namespace it used to have, while that is pending)
            v.extend(k.defs().into_iter().filter(|d| self.target_refusal(&d.spec.name).is_none()));
        }
        if let Some(p) = &self.platform {
            v.extend(p.defs());
        }
        if self.history.is_some() {
            v.push(tool("history_search", "Search what earlier steps and sessions of this workspace did (summaries, tool results, decisions, errors) when something is no longer in your context: `query`, optional `scope` (session or workspace, default workspace), optional `kind` (turn, tool, summary, decision, error), optional `k` (default 4). Read-only; the snippets are untrusted data with ids.".into(), obj(json!({"query": {"type": "string"}, "scope": {"type": "string"}, "kind": {"type": "string"}, "k": {"type": "integer"}}), &["query"]), Risk::Safe, true));
        }
        v.push(tool("board_add", format!("Add a new feature row to the board ({file}); if the workspace has no board yet this creates it. Use it first for any multi-step job: one planned row per step, then board_update to partial/done as you go. Give `prefix` (an area such as BROW; the next free number is chosen) or an `id`, a `title`, and optionally `phase`, `priority`, `status` (default planned) and `notes`. The person approves it."), obj(json!({"prefix": {"type": "string"}, "id": {"type": "string"}, "title": {"type": "string"}, "phase": {"type": "string"}, "priority": {"type": "string"}, "status": {"type": "string"}, "notes": {"type": "string"}, "kind": {"type": "string"}, "severity": {"type": "string"}, "repro": {"type": "string"}, "tests": {"type": "array", "items": {"type": "string"}}, "acceptance": {"type": "array", "items": {"type": "string"}}, "files": {"type": "array", "items": {"type": "string"}}}), &["title"]), Risk::Ask, false));
        v.push(tool("bug_report", format!("Report a bug: creates a `B-` row on the board ({file}) and captures the evidence into .iyakki/bugs/<id>/ (logs of every container, current and previous, pod descriptions, events, status with env values redacted, the manifest in effect, the Dockerfile, images, the last failing build or check). Use it for every failure instead of adding an F- row. `title`, `severity` (low, med, high), optional `repro`, `pods` (names; default all pods of the workspace namespace), `since` (for example 10m), `lines` (default 300 per container, at most 2000), `feature` (the F- row you were working on). The same failure is reported once. Credentials are redacted before anything is written."), obj(json!({"title": {"type": "string"}, "severity": {"type": "string"}, "repro": {"type": "string"}, "pods": {"type": "array", "items": {"type": "string"}}, "since": {"type": "string"}, "lines": {"type": "integer"}, "feature": {"type": "string"}}), &["title"]), Risk::Ask, false));
        v.push(tool("board_link_tests", format!("Link tests to a feature on the board ({file}): `id` and `tests` (names or paths such as `crate::module::test_name`, added to the row's Tests column; names already there are kept). Use it for tests of the last run that no feature lists, and for tests you wrote. The person approves it."), obj(json!({"id": {"type": "string"}, "tests": {"type": "array", "items": {"type": "string"}}}), &["id", "tests"]), Risk::Ask, false));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        // a cluster tool in a workspace that is not a service: refused plainly, before anything else looks at it
        if self.k8s.is_some() && crate::k8stools::K8sTools::handles(name) {
            if let Some(why) = self.target_refusal(name) {
                return Err(why);
            }
        }
        if let Some(why) = self.queued_behind(name, args) {
            return Err(why);
        }
        self.track.before(name, args);
        let r = self.call_core(name, args);
        self.report(name, args, &r);
        if name == "k8s_apply" && r.is_ok() {
            self.namespace_rows_done();
        }
        if name == "board_add" {
            if let Some(id) = r.as_ref().ok().and_then(|t| t.strip_prefix("added ")).and_then(|t| t.lines().next()) {
                self.track.own(id.trim());
            }
        }
        self.track.after(name, args, &r);
        // something changed in the world: parked rows may be retried
        if r.is_ok() && matches!(name, "write_file" | "run_command" | "deploy_build" | "deploy_load" | "k8s_apply") {
            self.world.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if name == "write_file" {
                if let Some(p) = args.get("path").and_then(Value::as_str) {
                    let mut w = self.written.lock().unwrap_or_else(|e| e.into_inner());
                    if !w.iter().any(|x| x == p) {
                        w.push(p.to_string());
                    }
                }
            }
        }
        // a failed build: quote the failing step and give the first lines of the Dockerfile with numbers, so a small model can fix line N
        if name == "deploy_build" {
            let failed = match &r {
                Err(e) => Some(e.clone()),
                Ok(text) if text.contains("FAILED") => Some(text.clone()),
                _ => None,
            };
            if let Some(out) = failed {
                let block = self.build_failure_block(args, &out);
                *self.diag.lock().unwrap_or_else(|e| e.into_inner()) = Some((String::new(), block.clone()));
                *self.last_failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(("deploy_build".to_string(), crate::agent::truncate(&out, 20_000)));
                return Err(format!("{}\n\n{block}", crate::agent::truncate(&out, 2500)));
            }
        }
        // facts of the last deploy, for the next run's PROJECT STATE (also after a restart)
        if let Ok(text) = &r {
            let first = text.lines().next().unwrap_or("");
            match name {
                "deploy_build" => {
                    if let Some(img) = text.split_whitespace().skip_while(|w| *w != "Built").nth(1) {
                        crate::projectstate::note(&self.root, "image", img);
                    }
                }
                "deploy_load" => crate::projectstate::note(&self.root, "image", first),
                "deploy_status" => {
                    crate::projectstate::note(&self.root, "deploy_status", first);
                    if let Some(u) = text.lines().find_map(|l| l.split("reachable at:").nth(1)) {
                        crate::projectstate::note(&self.root, "reachable", u.trim().split(" ;").next().unwrap_or("").trim());
                    }
                }
                _ => {}
            }
        }
        // what failed last (the only evidence of a local build), a pod that cannot start becomes one bug with its logs, and a healthy
        // rollout resolves the bugs the harness reported
        match (&r, name) {
            (Err(e), "deploy_build" | "k8s_apply" | "run_command" | "deploy_run_job" | "deploy_load") => *self.last_failure.lock().unwrap_or_else(|x| x.into_inner()) = Some((name.to_string(), crate::agent::truncate(e, 20_000))),
            (Ok(text), "deploy_status") => {
                if crate::boardtrack::all_ready(text) {
                    self.resolve_bugs_for("", text);
                } else if !crate::bugs::bad_reasons_in(text).is_empty() {
                    *self.last_failure.lock().unwrap_or_else(|x| x.into_inner()) = Some((name.to_string(), crate::agent::truncate(text, 20_000)));
                    if let Some(id) = self.auto_bug("", "the deployment", text) {
                        return Ok(format!("{text}\nBug {id}: the logs of every container are in .iyakki/bugs/{id}/"));
                    }
                }
            }
            _ => {}
        }
        // board tool results stay short: the model needs the outcome, not a dump
        if matches!(name, "board_update" | "board_add") {
            return r.map(|t| crate::agent::truncate(&t, 300)).map_err(|e| crate::agent::truncate(&e, 300));
        }
        r
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.diag.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        // the agent's own file edits and shell commands count like the same calls made through write_file and run_command
        let (n, a) = match name {
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                let Some(p) = args.get("file_path").or_else(|| args.get("notebook_path")).and_then(Value::as_str) else { return };
                let rel = std::path::Path::new(p).strip_prefix(&self.root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.to_string());
                ("write_file", json!({"path": rel}))
            }
            "Bash" => {
                let cmd = args.get("command").and_then(Value::as_str).unwrap_or("");
                let Some(words) = crate::extagent::bridge::segments(cmd).into_iter().last() else { return };
                ("run_command", json!({"program": words[0], "args": words[1..]}))
            }
            _ => return,
        };
        let r: Result<String, String> = if ok { Ok(String::new()) } else { Err(String::new()) };
        self.track.after(n, &a, &r);
        self.report(n, &a, &r);
        if ok {
            self.world.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == "write_file" {
                if let Some(p) = a.get("path").and_then(Value::as_str) {
                    let mut w = self.written.lock().unwrap_or_else(|e| e.into_inner());
                    if !w.iter().any(|x| x == p) {
                        w.push(p.to_string());
                    }
                }
            }
        }
        self.inner.note_external(name, args, ok);
    }

    fn begin_step(&self) {
        *self.step.lock().unwrap_or_else(|e| e.into_inner()) = StepState::default();
    }

    fn should_end_run(&self) -> Option<String> {
        self.step.lock().unwrap_or_else(|e| e.into_inner()).end.take()
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.track.unresolved()
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        // a workspace with a connected cluster never starts, stops or installs one, whatever tool the model tries
        let connected = self.k8s.as_ref().map(|k| k.profile.describe());
        let window = crate::webengine::host_session();
        match name {
            "run_command" | "terminal" => {
                if name == "terminal" {
                    return crate::devcluster::guard_line_with(window, connected.as_deref().or(crate::devcluster::active().as_deref()), args.get("command").and_then(Value::as_str).unwrap_or("")).err();
                }
                let prog = args.get("program").and_then(Value::as_str).unwrap_or("");
                let a: Vec<String> = args.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                crate::devcluster::guard_with(window, connected.as_deref().or(crate::devcluster::active().as_deref()), prog, &a).err()
            }
            "cluster_start" | "cluster_stop" | "install_cli" if connected.is_some() || window => {
                if name == "install_cli" && args.get("id").and_then(Value::as_str) != Some("k3s") {
                    return self.inner.refusal(name, args);
                }
                Some(crate::devcluster::block_message(&format!("{} a cluster", match name { "cluster_start" => "start", "cluster_stop" => "stop", _ => "install" }), connected.as_deref().or(crate::devcluster::active().as_deref())))
            }
            _ => self.inner.refusal(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
        let what = match name {
            "board_list" => "read the feature board".to_string(),
            "board_depends" => "read the board's dependencies".to_string(),
            n if crate::notetools::handles(n) => crate::notetools::describe(n, args),
            "bug_report" => format!("report the bug “{}” and capture its logs into .iyakki/bugs/", s("title")),
            "board_link_tests" => format!("link tests to feature {}: {}", s("id"), args.get("tests").map(|t| t.to_string()).unwrap_or_default()),
            "history_search" => format!("search the workspace history for “{}”", s("query")),
            "board_update" => format!("update feature {} on the board ({})", s("id"), ["status", "phase", "priority", "progress", "comment", "blocker"].iter().filter(|k| args.get(**k).is_some()).map(|k| format!("{k}: {}", args[*k].as_str().map(str::to_string).unwrap_or_else(|| args[*k].to_string()))).collect::<Vec<_>>().join(", ")),
            "board_add" => format!("add a feature to the board: {}", s("title")),
            n if crate::k8stools::K8sTools::handles(n) && self.k8s.is_some() => self.k8s.as_ref().expect("checked").describe(n, args),
            n if crate::platform::tools::handles(n) && self.platform.is_some() => self.platform.as_ref().expect("checked").describe(n, args),
            _ => self.inner.describe(name, args),
        };
        // an approval for a call that would be refused says so, so nobody approves it for nothing
        let refused = if matches!(name, "board_list" | "board_depends" | "board_update" | "board_add" | "board_link_tests" | "bug_report" | "history_search") || crate::notetools::handles(name) || crate::k8stools::K8sTools::handles(name) || crate::platform::tools::handles(name) {
            None
        } else if self.refusal(name, args).is_some() {
            Some(" — will be REFUSED: a cluster is already running and connected")
        } else {
            self.guard(name, args).err().map(|_| " — will be REFUSED: outside the workspace")
        };
        format!("[{}] {what}{}", self.name, refused.unwrap_or(""))
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "board_link_tests" => Some(match self.planned_link(args) {
                Ok((_, _, diff)) => diff,
                Err(e) => format!("this would be refused: {e}"),
            }),
            "board_update" => Some(match self.planned_update(args) {
                Ok((_, _, diff)) => diff,
                Err(e) => format!("this would be refused: {e}"),
            }),
            "board_add" => Some(match self.planned_add(args) {
                Ok((_, _, diff)) => diff,
                Err(e) => format!("this would be refused: {e}"),
            }),
            n if crate::k8stools::K8sTools::handles(n) && self.k8s.is_some() => self.k8s.as_ref().expect("checked").preview(n, args),
            n if crate::platform::tools::handles(n) && self.platform.is_some() => self.platform.as_ref().expect("checked").preview(n, args),
            _ => self.inner.preview(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        // removing an environment is never covered by a grant
        !matches!(name, "k8s_teardown" | "k8s_registry_cleanup" | "platform_deploy" | "platform_upgrade" | "platform_remove") && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        // writes to a cluster are approved one at a time; "always" lasts for reads and the other tools only
        !matches!(name, "k8s_apply" | "k8s_delete" | "k8s_rollout_restart" | "deploy_load" | "k8s_stop_env" | "k8s_teardown" | "k8s_registry_cleanup" | "platform_deploy" | "platform_upgrade" | "platform_remove") && self.inner.approved_always(name)
    }

    /// A session grant for a cluster tool covers that cluster and namespace only, and never a remote or production cluster.
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if let (true, Some(k)) = (crate::k8stools::K8sTools::handles(name), &self.k8s) {
            if let Some(why) = k.grant_blocker() {
                return Some(why);
            }
        }
        self.inner.grant_blocker(name, args)
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        match (crate::k8stools::K8sTools::handles(name), &self.k8s) {
            (true, Some(k)) => format!("{} @{}", crate::grants::scope_of(name, args), k.grant_target()),
            _ => self.inner.grant_scope(name, args),
        }
    }
}

/// The system prompt addition for a workspace chat.
pub fn workspace_prompt_with_cluster(name: &str, root: &Path, features: &str, cluster: Option<(&crate::devcluster::ClusterProfile, &str)>) -> String {
    let mut s = workspace_prompt(name, root, features);
    if let Some((p, ns)) = cluster {
        s.push_str(&crate::k8stools::prompt_line(p, ns));
    }
    s
}

pub fn workspace_prompt(name: &str, root: &Path, features: &str) -> String {
    format!("\nYou are working inside the workspace \"{name}\" (the AI IDE). Its root folder is {root}. Relative paths start there, and every file tool is limited to that folder: do not try to read or write elsewhere (it is refused). Other products have their own workspaces and chats. {features} in the root is the feature tracking board (a markdown table: id, feature, status, tests, notes); it is created by your first board_add if it does not exist. USE IT for every job of more than one step (build an app, add a feature, deploy): FIRST call board_add once per step with status planned (prefix = a short area such as APP, and a title), THEN do the work, and call board_update to partial (with a comment saying what is left) or done as each step progresses. When you write tests for a row, list them in the row (`tests`: real test names or paths such as `crate::module::test_name` or `tests/test_a.py::test_b`, never sentences; free-text acceptance criteria go in `acceptance`; the source files in `files`), and when the last run shows tests of the board that no feature lists, link them with board_link_tests. A bug or failure gets its own row with a B- id (`kind: bug`, never an F- row): call bug_report so the logs of the failing pods are captured, fix it, verify, and attach the passing run. Never mark a row done unless a test passed or you verified it (a command ran, a page answered) and say which in the comment; use board_list to see the rows before adding duplicates. Never edit the table by hand. Changes you propose to open files appear in the editor as reviewable hunks. Rows may wait for other rows (`depends`): board_depends lists the ready ones; start only those.{notes}", notes = crate::notetools::PROMPT, root = root.display())
}

/// Used by tests and the IDE to show the row text a diff would contain.
pub fn row_text(b: &Board, id: &str) -> Option<String> {
    b.get(id).map(|f| ProjectTools::fmt_row(&f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Inner {
        calls: Mutex<Vec<String>>,
    }
    impl Tools for Inner {
        fn defs(&self) -> Vec<ToolDef> {
            ["read_file", "write_file", "terminal", "run_command"].iter().map(|n| ToolDef { spec: ToolSpec { name: n.to_string(), description: String::new(), schema: json!({}) }, risk: Risk::Safe, untrusted: false }).collect()
        }
        fn call(&self, name: &str, args: &Value) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("{name} {args}"));
            Ok("done".into())
        }
        fn describe(&self, name: &str, _a: &Value) -> String {
            format!("inner {name}")
        }
    }

    const FEATURES: &str = "<!-- summary:start -->done 1 · partial 0 · planned 1 · blocked 0 · deprecated 0 · total 2<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-A-01 | Alpha | P1 | done | P0 | 100% | `c::t::a` |  |\n| F-A-02 | Beta thing | P2 | planned | P1 | 0% | — |  |\n";

    fn world(tag: &str) -> (ProjectTools, Arc<Inner>, PathBuf) {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-pt-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj")).unwrap();
        std::fs::create_dir_all(d.join("other")).unwrap();
        std::fs::write(d.join("other/secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(d.join("other"), d.join("proj/escape")).unwrap();
        std::fs::write(d.join("proj/FEATURES.md"), FEATURES).unwrap();
        let inner = Arc::new(Inner { calls: Mutex::new(vec![]) });
        let t = ProjectTools::new(inner.clone(), &d.join("proj"), "Shop", &d.join("proj/FEATURES.md"));
        (t, inner, d)
    }

    #[test]
    fn file_tools_are_refused_outside_the_root_including_dotdot_and_symlinks() {
        let (t, inner, d) = world("guard");
        assert!(t.call("read_file", &json!({"path": "FEATURES.md"})).is_ok());
        assert!(t.call("write_file", &json!({"path": "src/new.rs", "content": "x"})).is_ok());
        for bad in ["../other/secret.txt", "escape/secret.txt", "escape/new.txt", "/etc/passwd", &d.join("other/secret.txt").display().to_string()] {
            let e = t.call("read_file", &json!({"path": bad})).unwrap_err();
            assert!(e.contains("outside the workspace \"Shop\""), "{bad}: {e}");
        }
        assert!(t.call("run_command", &json!({"program": "cat", "args": ["../other/secret.txt"]})).unwrap_err().contains("outside"));
        assert!(t.call("run_command", &json!({"program": "cat", "args": ["/etc/passwd"]})).unwrap_err().contains("outside"));
        assert!(t.call("run_command", &json!({"program": "cat", "args": ["--file=/etc/passwd"]})).is_err());
        assert!(t.call("run_command", &json!({"program": "ls", "args": ["-la", "src"]})).is_ok());
        assert!(t.call("terminal", &json!({"command": "cat /etc/passwd"})).unwrap_err().contains("shell line"));
        assert!(!t.defs().iter().any(|d| d.spec.name == "terminal"));
        assert_eq!(inner.calls.lock().unwrap().len(), 3, "refused calls never reach the real tools");
        assert!(t.describe("write_file", &json!({"path": "a"})).starts_with("[Shop] inner write_file"), "approvals name the workspace");
    }

    #[test]
    fn board_tools_list_update_and_add_with_approval_and_a_row_diff() {
        let (t, _i, d) = world("board");
        let defs = t.defs();
        let risk = |n: &str| defs.iter().find(|x| x.spec.name == n).unwrap().risk;
        assert_eq!((risk("board_list"), risk("board_update"), risk("board_add")), (Risk::Safe, Risk::Ask, Risk::Ask));
        let l = t.call("board_list", &json!({})).unwrap();
        assert!(l.contains("2 features") && l.contains("F-A-02 | planned"), "{l}");
        assert!(t.call("board_list", &json!({"filter": "area:A"})).unwrap().contains("F-A-01"));
        assert!(!t.call("board_list", &json!({"filter": "done"})).unwrap().contains("F-A-02"));
        // partial needs a comment
        let e = t.call("board_update", &json!({"id": "F-A-02", "status": "partial"})).unwrap_err();
        assert!(e.contains("needs a comment"), "{e}");
        assert!(t.preview("board_update", &json!({"id": "F-A-02", "status": "partial"})).unwrap().contains("would be refused"));
        let args = json!({"id": "F-A-02", "status": "partial", "comment": "blocked by the new API"});
        let pv = t.preview("board_update", &args).unwrap();
        assert!(pv.contains("- F-A-02 | planned") && pv.contains("+ F-A-02 | partial") && pv.contains("50%"), "{pv}");
        assert_eq!(std::fs::read_to_string(d.join("proj/FEATURES.md")).unwrap(), FEATURES, "a preview writes nothing");
        assert!(t.describe("board_update", &args).starts_with("[Shop] update feature F-A-02"));
        let r = t.call("board_update", &args).unwrap();
        assert!(r.contains("updated F-A-02") && r.contains("needs at least one test") , "{r}");
        let text = std::fs::read_to_string(d.join("proj/FEATURES.md")).unwrap();
        assert!(text.lines().any(|l| l.starts_with("| F-A-02 | Beta thing | P2 | partial | P1 | 50% | — | 20") && l.ends_with("| | blocked by the new API |")), "Started is set, Finished stays empty: {text}");
        assert!(text.contains("done 1 · partial 1 · planned 0"), "totals kept: {text}");
        assert!(d.join("proj/FEATURES.md.bak").exists());
        let r = t.call("board_add", &json!({"prefix": "A", "title": "Gamma", "phase": "P3"})).unwrap();
        assert!(r.contains("added F-A-03"), "{r}");
        assert!(std::fs::read_to_string(d.join("proj/FEATURES.md")).unwrap().contains("| F-A-03 | Gamma | P3 | planned | P2 | 0% | — |  |"));
        assert!(t.call("board_add", &json!({"title": "no id"})).is_err());
        t.begin_step();
        assert!(t.call("board_update", &json!({"id": "F-NOPE-01", "status": "done"})).unwrap_err().contains("no feature"));
        assert!(t.call("board_update", &json!({"id": "F-A-01"})).unwrap_err().contains("nothing to change"));
    }

    #[test]
    fn without_a_board_the_tools_say_so() {
        let (t, _i, d) = world("noboard");
        std::fs::remove_file(d.join("proj/FEATURES.md")).unwrap();
        assert!(t.call("board_list", &json!({})).unwrap_err().contains("no features board"));
        assert!(workspace_prompt("Shop", &d.join("proj"), "FEATURES.md").contains("limited to that folder"));
    }

    #[test]
    fn kubectl_through_run_command_is_pinned_to_the_workspace_cluster() {
        use crate::devcluster::{ClusterProfile, Source};
        use crate::wsexec::FakeRunner;
        let d = std::env::temp_dir().join(format!("iyakki-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj")).unwrap();
        let inner = Arc::new(Inner { calls: Mutex::new(vec![]) });
        let p = ClusterProfile { name: "k3s".into(), kubeconfig: "/etc/rancher/k3s/k3s.yaml".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::Project, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
        let k = Arc::new(crate::k8stools::K8sTools::new(Arc::new(FakeRunner::new(&["kubectl"])), p, "shop-dev", "Shop", &d.join("proj"), Arc::new(|_, _| {})));
        let t = ProjectTools::new(inner.clone(), &d.join("proj"), "Shop", &d.join("proj/FEATURES.md")).with_k8s(k);
        t.call("run_command", &json!({"program": "kubectl", "args": ["get", "pods", "--context", "remote", "--server=https://192.168.1.68:6443"]})).unwrap();
        t.call("run_command", &json!({"program": "ls", "args": ["-la"]})).unwrap();
        let calls = inner.calls.lock().unwrap().clone();
        assert!(calls[0].contains("--kubeconfig\",\"/etc/rancher/k3s/k3s.yaml\",\"--context\",\"default\",\"get\",\"pods\"") && !calls[0].contains("192.168.1.68") && !calls[0].contains("remote"), "{}", calls[0]);
        assert!(!calls[1].contains("kubeconfig"), "{}", calls[1]);
    }

    #[test]
    fn a_namespace_row_is_done_once_an_apply_worked_even_if_it_was_marked_blocked() {
        use crate::devcluster::{ClusterProfile, Source};
        use crate::wsexec::FakeRunner;
        let d = std::env::temp_dir().join(format!("iyakki-nsrow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj")).unwrap();
        let board = "<!-- summary:start -->done 0 · partial 0 · planned 1 · blocked 1 · deprecated 0 · total 2<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-APP-01 | Create namespace shop-dev | P1 | blocked | P1 | 0% | — | cannot create the cluster-scoped namespace |\n| F-APP-02 | Deploy the web app | P1 | planned | P1 | 0% | — |  |\n";
        std::fs::write(d.join("proj/FEATURES.md"), board).unwrap();
        let r = Arc::new(FakeRunner::new(&["kubectl"]));
        r.answer("diff -f", "");
        r.answer("get namespace shop-dev", "namespace/shop-dev");
        r.answer("apply -f", "deployment.apps/web created");
        let p = ClusterProfile { name: "k3s".into(), kubeconfig: "/k".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::Project, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
        let k = Arc::new(crate::k8stools::K8sTools::new(r, p, "shop-dev", "Shop", &d.join("proj"), Arc::new(|_, _| {})));
        let t = ProjectTools::new(Arc::new(Inner { calls: Mutex::new(vec![]) }), &d.join("proj"), "Shop", &d.join("proj/FEATURES.md")).with_k8s(k);
        let yaml = "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: shop-dev\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  template:\n    spec:\n      containers:\n      - name: web\n        image: web:1\n        resources:\n          limits:\n            cpu: 100m\n        securityContext:\n          runAsNonRoot: true\n";
        let out = t.call("k8s_apply", &json!({"yaml": yaml})).unwrap();
        assert!(out.contains("skipped Namespace shop-dev") && out.contains("deployment.apps/web created"), "{out}");
        let text = std::fs::read_to_string(d.join("proj/FEATURES.md")).unwrap();
        assert!(text.lines().any(|l| l.starts_with("| F-APP-01 |") && l.contains("| done |")), "{text}");
        assert!(text.lines().any(|l| l.starts_with("| F-APP-02 |") && l.contains("| planned |")), "the other row is untouched: {text}");
    }

    fn calc_world(tag: &str) -> (ProjectTools, std::path::PathBuf, Arc<crate::wsexec::FakeRunner>) {
        use crate::devcluster::{ClusterProfile, Source};
        let d = std::env::temp_dir().join(format!("iyakki-calc-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("deploy")).unwrap();
        // the owner's workspace: a web service with k3s rows
        std::fs::write(d.join("server.js"), "require('http').createServer(()=>{}).listen(8080)").unwrap();
        std::fs::write(d.join("Dockerfile"), "FROM node:20\n").unwrap();
        std::fs::write(d.join("package.json"), "{}").unwrap();
        let board = "<!-- summary:start -->done 0 · partial 0 · planned 3 · blocked 0 · deprecated 0 · total 3<!-- summary:end -->\n\n| ID | Feature | Phase | Status | Pri | Progress | Tests | Notes |\n|---|---|---|---|---|---|---|---|\n| F-APP-01 | Calculator web app titled calc (UI + /api/calc) | P1 | planned | P1 | 0% | — |  |\n| F-DEPLOY-01 | Deploy calc to k3s (calc-dev) and verify | P1 | planned | P1 | 0% | — |  |\n| F-TEST-01 | Integration tests as a Job after rollout | P1 | planned | P1 | 0% | — |  |\n";
        std::fs::write(d.join("FEATURES.md"), board).unwrap();
        let r = Arc::new(crate::wsexec::FakeRunner::new(&["kubectl"]));
        let p = ClusterProfile { name: "k3s".into(), kubeconfig: "/k".into(), context: "default".into(), server: "https://127.0.0.1:6443".into(), source: Source::Project, exists: true, readable: true, tested: true, reachable: true, k3s: true, local: true, version: String::new(), can_i: String::new(), problem: String::new(), fix: String::new() };
        let k = Arc::new(crate::k8stools::K8sTools::new(r.clone(), p, "calc-dev", "calc", &d, Arc::new(|_, _| {})));
        let t = ProjectTools::new(Arc::new(Inner { calls: Mutex::new(vec![]) }), &d, "calc", &d.join("FEATURES.md")).with_k8s(k);
        (t, d, r)
    }

    fn names(t: &ProjectTools) -> Vec<String> {
        t.defs().into_iter().map(|d| d.spec.name).collect()
    }

    #[test]
    fn a_tile_app_workspace_has_no_deploy_tools_refuses_them_and_takes_no_cluster_rows() {
        use crate::target::{self, Target};
        let (t, d, r) = calc_world("tile");
        // before a target exists the older workspace keeps its tools
        assert!(names(&t).contains(&"deploy_build".to_string()));
        // the owner's exact request resolves to a tile app, and the workspace is set up for it
        let prompt = r#"create a iyakki calculator app deploy as a title "calc""#;
        assert_eq!(target::resolve_intent(prompt), target::Intent::Is(Target::IyakkiTile));
        target::write(&d, "calc", Target::IyakkiTile).unwrap();
        let n = names(&t);
        assert!(!n.iter().any(|x| crate::k8stools::K8sTools::handles(x)), "no cluster tool is offered: {n:?}");
        assert!(n.contains(&"board_add".to_string()) && n.contains(&"write_file".to_string()) || n.contains(&"board_add".to_string()));
        // a call that gets through anyway is refused with the words the owner asked for
        for tool in ["deploy_build", "deploy_load", "k8s_apply", "k8s_delete", "deploy_status", "deploy_run_job", "k8s_teardown"] {
            let e = t.call(tool, &json!({"image": "x", "dir": ".", "yaml": "kind: Pod"})).unwrap_err();
            assert_eq!(e, "this workspace targets an IYAKKI tile app; change the target to deploy to k3s", "{tool}");
        }
        assert!(r.calls().is_empty(), "nothing reached the cluster");
        // cluster rows are refused on the board; tile rows are fine
        for title in ["Deploy the calculator to k3s", "Push the image to the registry", "Run integration tests as a Job after the rollout", "Write the Dockerfile"] {
            let e = t.call("board_add", &json!({"prefix": "X", "title": title})).unwrap_err();
            assert!(e.contains("does not belong on this board"), "{title}: {e}");
        }
        for (_, title) in target::board_template(Target::IyakkiTile) {
            assert!(t.call("board_add", &json!({"prefix": "TILE", "title": title})).is_ok(), "{title}");
        }
        // the project state tells the assistant what the target is and pins the tile skill
        let state = crate::projectstate::build(&crate::projectstate::Input { name: "calc", root: &d, features: &d.join("FEATURES.md"), stack_override: "", test: "", run: "", build: "", cluster: Some("k3s · namespace calc-dev".into()), history: None });
        assert!(state.contains("Target: IYAKKI tile app") && state.contains("NO cluster work") && !state.contains("cluster k3s"), "{state}");
        assert!(!state.contains("A Dockerfile exists"), "{state}");
        let st = crate::projectstate::detect_stack(&d, "");
        assert_eq!(st.pinned_skills("add a clear button"), vec!["lang-javascript", "iyakki-app-author", "iyakki-tile-author"], "no k3s practices for a tile app even with a Dockerfile around");
        // switching back to a service brings the tools back
        target::write(&d, "calc", Target::K3sService).unwrap();
        assert!(names(&t).contains(&"deploy_build".to_string()));
        assert!(crate::projectstate::detect_stack(&d, "").pinned_skills("hi").contains(&"k3s-dev-best-practices"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn changing_the_target_of_the_owners_calc_workspace_plans_the_conversion_and_offers_only_the_teardown() {
        use crate::target::{self, Target};
        let (t, d, r) = calc_world("change");
        let plan = target::plan_change(&d, "calc", Target::IyakkiTile);
        assert_eq!(plan.drop_files, vec!["server.js", "Dockerfile", "deploy/"]);
        assert_eq!(plan.drop_rows.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["F-DEPLOY-01", "F-TEST-01"], "the UI row stays");
        assert_eq!(plan.teardown_namespace.as_deref(), Some("calc-dev"));
        assert!(plan.summary.contains("server.js") && plan.summary.contains("calc-dev") && plan.summary.contains("asks first"), "{}", plan.summary);
        assert_eq!(std::fs::read_to_string(d.join("server.js")).unwrap().len() > 0, true, "planning deletes nothing");
        assert!(target::read(&d).is_none());
        let (_, msg) = target::apply_change(&d, "calc", Target::IyakkiTile).unwrap();
        assert!(msg.contains("ask_user") && msg.contains("k8s_teardown") && msg.contains("calc-dev") && msg.contains("F-DEPLOY-01"), "{msg}");
        assert_eq!(target::read(&d), Some(Target::IyakkiTile));
        assert!(d.join("server.js").exists() && d.join("Dockerfile").exists(), "files change only through the assistant's own approved tools");
        assert!(target::apply_change(&d, "calc", Target::IyakkiTile).is_err(), "already that target");
        // only the teardown is offered, to remove the old namespace; everything else is refused
        let n = names(&t);
        assert_eq!(n.iter().filter(|x| crate::k8stools::K8sTools::handles(x)).collect::<Vec<_>>(), vec!["k8s_teardown"], "{n:?}");
        assert!(t.call("k8s_apply", &json!({"yaml": "x"})).unwrap_err().contains("targets an IYAKKI tile app"));
        assert!(crate::agent::hard_ask("k8s_teardown", &json!({})).is_some(), "the teardown always asks, in every mode");
        // once the namespace is gone the offer ends
        target::teardown_done(&d);
        assert!(!names(&t).iter().any(|x| crate::k8stools::K8sTools::handles(x)));
        let _ = r;
        let _ = std::fs::remove_dir_all(&d);
    }
}
