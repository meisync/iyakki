//! Autopilot with an external coding agent (a fake `claude` that speaks MCP to the bridge): the harness keeps the board, the
//! checks, the caps and the summary; each step's message is the agent's prompt, resuming its own session.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::fake::{claude_mcp, tmp};
use super::run::{start, RunRequest};
use super::*;
use crate::agent::{Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::autopilot::{self, Executor, StepCall};
use crate::chat::{Message, ToolSpec};

fn have_python() -> bool {
    let ok = crate::wsexec::on_path("python3");
    if !ok {
        eprintln!("skipped: python3 is not installed (the fake Claude Code speaks MCP from a small python script)");
    }
    ok
}

struct Inner {
    root: std::path::PathBuf,
}

fn def(name: &str, risk: Risk) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: format!("the {name} tool"), schema: json!({"type": "object"}) }, risk, untrusted: false }
}

impl Tools for Inner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![def("read_file", Risk::Safe), def("write_file", Risk::Ask), def("deploy_build", Risk::Ask), def("deploy_status", Risk::Safe), def("k8s_delete", Risk::Ask), def("run_command", Risk::Ask)]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == "write_file" {
            let p = args.get("path").and_then(Value::as_str).unwrap_or("x.txt");
            std::fs::write(self.root.join(p), "<html><audio></audio></html>").map_err(|e| e.to_string())?;
        }
        Ok(match name {
            "deploy_status" => "Deployment web: 1/1 ready, Healthy".to_string(),
            o => format!("{o} ok"),
        })
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        format!("{name} {args}")
    }
}

struct World {
    root: std::path::PathBuf,
    dir: std::path::PathBuf,
    tools: Arc<crate::boardtools::ProjectTools>,
    session: Arc<Mutex<Option<String>>>,
    usage: Arc<Mutex<Vec<(u64, u64)>>>,
}

fn world(tag: &str, plans: Value) -> World {
    let dir = tmp(tag);
    let root = dir.join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(dir.join("plan.json"), plans.to_string()).unwrap();
    claude_mcp(&dir);
    let tools = Arc::new(crate::boardtools::ProjectTools::new(Arc::new(Inner { root: root.clone() }), &root, "Web", &root.join("FEATURES.md")));
    World { root, dir, tools, session: Default::default(), usage: Default::default() }
}

impl World {
    fn executor(&self, allow_auto: bool) -> Executor {
        let agent: Arc<dyn ExternalAgent> = Arc::new(claude_code::ClaudeCode::new(AgentCfg { command: self.dir.join("claude").display().to_string(), ..Default::default() }));
        let (root, tools, session, usage) = (self.root.clone(), self.tools.clone(), self.session.clone(), self.usage.clone());
        Arc::new(move |call: StepCall| -> RunHandle {
            let resume = session.lock().unwrap().clone();
            let (s2, u2) = (session.clone(), usage.clone());
            let prepare = call.prepare.clone();
            let t2 = tools.clone();
            start(RunRequest {
                agent: agent.clone(),
                workspace: root.clone(),
                prompt: call.prompt,
                system: Arc::new(move || match &prepare {
                    Some(p) => p(&t2.defs()).iter().filter(|m| m.role == crate::chat::Role::System).map(|m| m.content.clone()).collect::<Vec<_>>().join("\n"),
                    None => String::new(),
                }),
                model: String::new(),
                resume,
                tools: tools.clone(),
                limits: call.limits,
                allow_auto,
                key: None,
                audit: Arc::new(|_, _| {}),
                exe: "/nonexistent/iyakki".into(),
                on_session: Arc::new(move |s| *s2.lock().unwrap() = Some(s.to_string())),
                on_usage: Arc::new(move |_, _, i, o| u2.lock().unwrap().push((i, o))),
                no_tools: false,
            })
        })
    }

    fn run(&self, allow_auto: bool, limits: RunLimits, goal: &str, verdict: impl FnMut(&str) -> Verdict) -> Vec<RunEvent> {
        let mut ctx = autopilot::Context::new(&self.root, &self.root.join("FEATURES.md"), goal);
        ctx.executor = Some(self.executor(allow_auto));
        ctx.state = Some(Arc::new(|| "PROJECT STATE\nstack: node".to_string()));
        let h = autopilot::start(Arc::new(crate::chat::Unconfigured), self.tools.clone(), vec![Message::system("SYS"), Message::user(goal)], limits, crate::assistrun::Assist::off(), ctx);
        drive(&h, verdict)
    }

    fn args(&self, n: usize) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(format!("args{n}.txt"))).unwrap().lines().map(str::to_string).collect()
    }
    /// The prompt (the arguments after `--`; the file has one line per argument line).
    fn prompt(&self, n: usize) -> String {
        let a = self.args(n);
        let at = a.iter().position(|x| x == "--").expect("the request follows --");
        a[at + 1..].join("\n")
    }
    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.join("count")).map_or(0, |s| s.trim().parse().unwrap_or(0))
    }
}

fn drive(h: &RunHandle, mut verdict: impl FnMut(&str) -> Verdict) -> Vec<RunEvent> {
    let mut out = vec![];
    let end = Instant::now() + Duration::from_secs(90);
    while Instant::now() < end {
        if let Ok(e) = h.events.recv_timeout(Duration::from_millis(50)) {
            if let RunEvent::Approval { id, name, .. } = &e {
                h.decide(id, verdict(name));
            }
            let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
            out.push(e);
            if done {
                return out;
            }
        }
    }
    panic!("autopilot never ended: {out:?}");
}

fn final_text(ev: &[RunEvent]) -> String {
    ev.iter().rev().find_map(|e| if let RunEvent::Final { text } = e { Some(text.clone()) } else { None }).unwrap_or_else(|| panic!("no final text in {ev:?}"))
}

fn notes(ev: &[RunEvent]) -> Vec<String> {
    ev.iter().filter_map(|e| if let RunEvent::Note { text } = e { Some(text.clone()) } else { None }).collect()
}

fn lim() -> RunLimits {
    RunLimits { max_steps: 20, max_tool_calls: 100, max_seconds: 120, approval_timeout: Duration::from_secs(30), ..Default::default() }
}

fn add(title: &str) -> Value {
    json!({"name": "board_add", "args": {"prefix": "APP", "title": title}})
}

/// The agent's own Write tool (it asks IYAKKI first through the permission tool, then writes the file itself).
fn write(path: &str) -> Value {
    json!({"approve": {"tool_name": "Write", "input": {"file_path": path, "content": "<html><audio></audio></html>"}}})
}

#[test]
fn a_multi_step_goal_completes_with_one_vendor_session_resumed_each_step() {
    if !have_python() {
        return;
    }
    let w = world(
        "ap1",
        json!([
            {"calls": [add("Write server.js with an HTTP server"), add("Add a Dockerfile")], "text": "Planned two steps.", "cost": 0.01},
            {"calls": [write("server.js")], "text": "Wrote server.js.", "cost": 0.01},
            {"calls": [{"approve": {"tool_name": "Bash", "input": {"command": "ls"}}}, write("Dockerfile")], "text": "Wrote the Dockerfile.", "cost": 0.01},
            {"calls": [{"name": "deploy_status", "args": {"name": "web"}}], "text": "Verified: the deployment is ready. Done: two rows. You: nothing.", "cost": 0.01}
        ]),
    );
    let ev = w.run(true, lim(), "create a simple nodejs webserver", |_| Verdict::Approve);
    let text = final_text(&ev);
    assert!(text.contains("Autopilot finished after 4 steps: 2 of 2 rows done"), "{text}\n{:?}", notes(&ev));
    // what ran, and only that
    assert!(text.contains("Files changed: server.js, Dockerfile") && text.contains("Commands run: ls"), "{text}");
    // one session: the first step starts it, every later step resumes it
    assert_eq!(w.runs(), 4);
    assert!(!w.args(1).iter().any(|a| a == "--resume"));
    for n in 2..=4 {
        let a = w.args(n);
        let at = a.iter().position(|x| x == "--resume").unwrap_or_else(|| panic!("step {n} does not resume: {a:?}"));
        assert_eq!(a[at + 1], "sess-ap");
        assert!(a.iter().any(|x| x == "--max-turns"), "a per-step cap of turns");
    }
    // the step message is the prompt: the goal, the current row, the PROJECT STATE
    let p2 = w.prompt(2);
    assert!(p2.starts_with("AUTOPILOT step 2: 0/2 rows done. Work on F-APP-01") && p2.contains("PROJECT STATE") && !p2.contains("F-APP-02"), "{p2}");
    // the harness's board is the plan; vendor tokens (without cache reads) are counted
    let board = std::fs::read_to_string(w.root.join("FEATURES.md")).unwrap();
    assert!(board.contains("done 2"), "{board}");
    assert_eq!(w.usage.lock().unwrap().len(), 4);
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Usage { tokens, cost_cents, .. } if *tokens >= 480 && *cost_cents > 3.9)), "cost and tokens add up over the run: {:?}", ev.iter().filter(|e| matches!(e, RunEvent::Usage { .. })).collect::<Vec<_>>());
    assert!(notes(&ev).iter().any(|n| n.starts_with("autopilot: step 2 · ")), "{:?}", notes(&ev));
}

#[test]
fn a_failed_check_hands_its_diagnosis_to_the_next_step_of_the_agent() {
    if !have_python() {
        return;
    }
    let w = world(
        "ap2",
        json!([
            {"calls": [add("Create MP3 Player and Picture Viewer"), add("Add a Dockerfile")], "text": "Planned."},
            {"calls": [{"name": "board_update", "args": {"id": "F-APP-01", "status": "done", "comment": "everything is ready for use"}}], "text": "Claimed it done."},
            {"calls": [write("player.html"), {"name": "board_update", "args": {"id": "F-APP-01", "status": "done", "comment": "the page exists"}}], "text": "Wrote the page."},
            {"calls": [write("Dockerfile")], "text": "Wrote the Dockerfile."},
            {"calls": [], "text": "Summary: both rows done."}
        ]),
    );
    let ev = w.run(true, lim(), "work on the features tasks", |_| Verdict::Approve);
    let p3 = w.prompt(3);
    assert!(p3.contains("The check for F-APP-01 FAILED. Diagnosis:") && p3.contains("Do not describe a fix: make it with tools"), "{p3}");
    let text = final_text(&ev);
    assert!(text.contains("2 of 2 rows done"), "{text}");
    assert!(crate::boardtrack::Tracker::new(&w.root, &w.root.join("FEATURES.md")).is_verified("F-APP-01"), "verified by the harness, not by the agent's say-so");
}

#[test]
fn the_cost_the_vendor_reports_stops_the_run_at_the_policy_cap() {
    if !have_python() {
        return;
    }
    let w = world(
        "ap3",
        json!([
            {"calls": [add("Add a Dockerfile"), add("Add a README")], "text": "Planned.", "cost": 0.02},
            {"calls": [write("Dockerfile")], "text": "Wrote it.", "cost": 0.02},
            {"calls": [write("README.md")], "text": "Wrote it.", "cost": 0.02}
        ]),
    );
    let ev = w.run(true, RunLimits { max_cost_cents: 3, ..lim() }, "add docs", |_| Verdict::Approve);
    assert!(matches!(ev.last(), Some(RunEvent::Stopped { reason }) if reason.contains("cost budget used")), "{:?}", ev.last());
    assert_eq!(w.runs(), 2, "no third step was started");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Model { text, .. } if text.contains("cost budget"))), "the stop is explained, nothing is lost");
    assert!(std::fs::read_to_string(w.root.join(".iyakki/autopilot.json")).unwrap().contains("stopped"));
}

#[test]
fn without_auto_every_action_asks_and_with_auto_only_what_hard_ask_names_does() {
    if !have_python() {
        return;
    }
    let plans = json!([
        {"calls": [add("Add a Dockerfile")], "text": "Planned."},
        {"calls": [{"name": "k8s_delete", "args": {"path": "old"}}, write("Dockerfile")], "text": "Handled the old deployment and wrote the Dockerfile."},
        {"calls": [], "text": "Summary."}
    ]);
    // Auto not allowed for this agent: the write asks too, and the run waits on the person
    let w = world("ap4", plans.clone());
    let asked = Mutex::new(vec![]);
    let ev = w.run(false, lim(), "add a dockerfile", |n| {
        asked.lock().unwrap().push(n.to_string());
        Verdict::Approve
    });
    assert_eq!(*asked.lock().unwrap(), ["board_add", "k8s_delete", "Write"]);
    assert!(notes(&ev).iter().any(|n| n.contains("Auto is off for Claude Code")), "{:?}", notes(&ev));
    assert!(notes(&ev).iter().any(|n| n.contains("needs you")), "the pause says so");
    assert!(final_text(&ev).contains("1 of 1 rows done"));
    // Auto allowed: the destructive tool still asks, the write does not
    let w = world("ap4b", plans.clone());
    let asked = Mutex::new(vec![]);
    w.run(true, lim(), "add a dockerfile", |n| {
        asked.lock().unwrap().push(n.to_string());
        Verdict::Approve
    });
    assert_eq!(*asked.lock().unwrap(), ["k8s_delete"]);
    // a refusal blocks the row
    let w = world("ap4c", json!([
        {"calls": [add("Add a Dockerfile")], "text": "Planned."},
        {"calls": [{"name": "k8s_delete", "args": {"path": "old"}}], "text": "Tried to clean up the old deployment."},
        {"calls": [], "text": "Summary."}
    ]));
    let ev = w.run(true, lim(), "add a dockerfile", |n| if n == "k8s_delete" { Verdict::Deny } else { Verdict::Approve });
    let board = std::fs::read_to_string(w.root.join("FEATURES.md")).unwrap();
    assert!(board.contains("blocked"), "{board}");
    assert!(notes(&ev).iter().any(|n| n.contains("F-APP-01 blocked")), "{:?}", notes(&ev));
    assert!(final_text(&ev).contains("1 blocked"), "{}", final_text(&ev));
}

#[test]
fn the_summary_lists_what_ran_and_shows_advice_as_suggested_not_run() {
    if !have_python() {
        return;
    }
    let w = world(
        "ap5",
        json!([
            {"calls": [add("Add a Dockerfile")], "text": "Planned."},
            {"calls": [{"approve": {"tool_name": "Bash", "input": {"command": "rm -rf build"}}}, write("Dockerfile")], "text": "Wrote the Dockerfile. To clean up later run `kubectl delete ns demo` yourself."},
            {"calls": [], "text": "Summary: done. To clean up later run `kubectl delete ns demo`."}
        ]),
    );
    // the person refuses the destructive shell command
    let ev = w.run(true, lim(), "add a dockerfile", |n| if n == "Bash" { Verdict::Deny } else { Verdict::Approve });
    let text = final_text(&ev);
    assert!(text.contains("Files changed: Dockerfile"), "{text}");
    assert!(!text.contains("rm -rf"), "a command that was refused is not listed as run: {text}");
    assert!(text.contains("Suggested, not run: kubectl delete ns demo"), "{text}");
}
