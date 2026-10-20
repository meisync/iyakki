use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::bridge::{classify, inside, shell_refusal, Bridge, Class, Correlator, Decisions, Gate};
use super::claude_code::ClaudeCode;
use super::copilot::Copilot;
use super::fake::{claude, copilot, texty, tmp};
use super::generic::Generic;
use super::run::{start, RunRequest, SessionMap};
use super::*;
use crate::agent::{Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::chat::ToolSpec;
use crate::wsexec::System;

// ------------------------------------------------------------------------------------------------ helpers

#[derive(Default)]
struct Stub {
    ran: Mutex<Vec<String>>,
}

fn def(name: &str, risk: Risk) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: format!("the {name} tool"), schema: json!({"type": "object"}) }, risk, untrusted: false }
}

impl Tools for Stub {
    fn defs(&self) -> Vec<ToolDef> {
        vec![
            def("board_list", Risk::Safe),
            def("board_add", Risk::Ask),
            def("k8s_delete", Risk::Ask),
            def("k8s_apply", Risk::Ask),
            def("vault_request", Risk::Ask),
            def("write_file", Risk::Ask),
            def("browser_open", Risk::Ask),
            def("run_command", Risk::Ask),
        ]
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.ran.lock().unwrap().push(format!("{name} {args}"));
        Ok(format!("{name} ran"))
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        format!("{name} {args}")
    }
    fn refusal(&self, name: &str, _args: &Value) -> Option<String> {
        (name == "k8s_apply").then(|| "production is read-only".to_string())
    }
}

type Log = Arc<Mutex<Vec<String>>>;

struct Rig {
    ws: PathBuf,
    dir: PathBuf,
    audit: Log,
    sessions: Log,
    usage: Arc<Mutex<Vec<(String, String, u64, u64)>>>,
    stub: Arc<Stub>,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = tmp(tag);
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        Rig { ws: ws.canonicalize().unwrap(), dir, audit: Default::default(), sessions: Default::default(), usage: Default::default(), stub: Arc::new(Stub::default()) }
    }

    fn request(&self, agent: Arc<dyn ExternalAgent>, prompt: &str) -> RunRequest {
        let (audit, sessions, usage) = (self.audit.clone(), self.sessions.clone(), self.usage.clone());
        RunRequest {
            agent,
            workspace: self.ws.clone(),
            prompt: prompt.into(),
            system: Arc::new(|| "PROJECT STATE\nstate here".into()),
            model: String::new(),
            resume: None,
            tools: self.stub.clone(),
            limits: RunLimits { approval_timeout: Duration::from_secs(5), max_seconds: 30, ..Default::default() },
            allow_auto: false,
            key: None,
            audit: Arc::new(move |k, d| audit.lock().unwrap().push(format!("{k} {d}"))),
            exe: PathBuf::from("/nonexistent/iyakki"),
            on_session: Arc::new(move |s| sessions.lock().unwrap().push(s.to_string())),
            on_usage: Arc::new(move |p, m, i, o| usage.lock().unwrap().push((p.into(), m.into(), i, o))),
            no_tools: false,
        }
    }

    fn claude(&self) -> Arc<dyn ExternalAgent> {
        let p = claude(&self.dir);
        Arc::new(ClaudeCode::new(AgentCfg { command: p.display().to_string(), ..Default::default() }))
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("args.txt")).unwrap().lines().map(str::to_string).collect()
    }
}

fn collect(h: &RunHandle) -> Vec<RunEvent> {
    let mut v = vec![];
    while let Ok(e) = h.events.recv_timeout(Duration::from_secs(20)) {
        let end = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
        v.push(e);
        if end {
            return v;
        }
    }
    panic!("the run never ended: {v:?}");
}

fn failed(ev: &[RunEvent]) -> String {
    ev.iter().find_map(|e| if let RunEvent::Failed { error } = e { Some(error.clone()) } else { None }).unwrap_or_else(|| panic!("no failure in {ev:?}"))
}

// ------------------------------------------------------------------------------------------------ detection

#[test]
fn detection_reports_installed_version_and_sign_in_state() {
    let rig = Rig::new("detect");
    let a = rig.claude();
    let s = detect(a.as_ref(), &System, &rig.ws);
    assert!(s.installed && s.ready() && s.version.contains("2.1.999"), "{s:?}");
    assert_eq!(s.auth, Auth::SignedIn("claude.ai".into()));
    assert_eq!((s.mode, s.chip.as_str()), (Mode::Bridged, "ready"));
    // signed out: the chip says so and the fix names the vendor's own login command
    std::fs::write(rig.dir.join("signedout"), "").unwrap();
    let s = detect(a.as_ref(), &System, &rig.ws);
    assert!(s.installed && !s.ready() && s.auth == Auth::SignedOut && s.chip == "sign in", "{s:?}");
    assert!(s.fix.contains("auth login") && s.fix.contains("never sees your password"), "{}", s.fix);
    // a key from the vault counts as signed in
    let keyed: Arc<dyn ExternalAgent> = Arc::new(ClaudeCode::new(AgentCfg { command: a.program(), key_ref: "vault:anthropic".into(), ..Default::default() }));
    assert_eq!(detect(keyed.as_ref(), &System, &rig.ws).auth, Auth::Key);
    // not installed: nothing is started, the fix shows the exact install command
    let none = ClaudeCode::new(AgentCfg { command: "/nonexistent/claude-xyz".into(), ..Default::default() });
    let s = detect(&none, &System, &rig.ws);
    assert!(!s.installed && !s.ready() && s.chip == "not installed", "{s:?}");
    assert!(s.fix.contains("npm install -g @anthropic-ai/claude-code") && s.fix.contains("asks first"), "{}", s.fix);
}

#[test]
fn copilot_and_custom_agents_are_detected_and_honest_about_what_they_can_be_held_to() {
    let rig = Rig::new("detect2");
    let c = Copilot::new(AgentCfg { command: copilot(&rig.dir).display().to_string(), ..Default::default() });
    let s = detect(&c, &System, &rig.ws);
    assert!(s.installed && s.version.contains("1.0.0") && s.auth == Auth::Unknown && s.mode == Mode::Bridged, "{s:?}");
    assert!(Copilot::new(AgentCfg { advisor_only: true, ..Default::default() }).mode() == Mode::Advisor);
    assert!(Mode::Advisor.label().contains("you apply edits"));
    let g = Generic::new(CustomCfg { id: "aider".into(), name: "Aider".into(), command: "no-such-aider-xyz --yes".into(), ..Default::default() });
    let s = detect(&g, &System, &rig.ws);
    assert!(!s.installed && s.fix.contains("no-such-aider-xyz"), "{s:?}");
    assert_eq!(g.id(), "custom:aider");
}

#[test]
fn settings_validate_and_the_model_list_names_agents() {
    let mut a = AgentsCfg::default();
    assert!(a.validate().is_ok());
    assert!(!a.claude.allow_auto && !a.copilot.allow_auto, "Auto is off until the person turns them on");
    a.custom.push(CustomCfg { id: "bad id".into(), command: "x".into(), ..Default::default() });
    assert!(a.validate().is_err());
    a.custom[0].id = "aider".into();
    a.custom[0].key_env = "lower".into();
    assert!(a.validate().is_err());
    a.custom[0].key_env = "OPENAI_API_KEY".into();
    assert!(a.validate().is_ok());
    a.claude.key_ref = "plain-key".into();
    assert!(a.validate().unwrap_err().contains("vault"), "a key can only come from the vault");
    assert_eq!(provider_of("claude-code"), "agent:claude-code");
    assert_eq!(agent_of("agent:copilot"), Some("copilot"));
    assert_eq!(agent_of("ollama"), None);
    assert!(a.agent("custom:aider").is_some() && a.agent("custom:nope").is_none() && a.agent("copilot").is_some());
    assert_eq!(a.all().len(), 3);
    // it round-trips through the settings file
    let mut s = crate::settings::Settings::default();
    s.agents = AgentsCfg::default();
    s.agents.custom.push(CustomCfg { id: "x".into(), command: "x {prompt}".into(), ..Default::default() });
    let back: crate::settings::Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.agents, s.agents);
}

// ------------------------------------------------------------------------------------------------ event mapping

#[test]
fn claude_stream_json_lines_become_events() {
    let a = ClaudeCode::new(AgentCfg::default());
    let mut st = ParseState::default();
    let p = |l: &str, st: &mut ParseState| a.parse_line(l, st);
    assert_eq!(p(r#"{"type":"system","subtype":"init","session_id":"s1","model":"m1"}"#, &mut st), vec![Ev::Init { session: "s1".into(), model: "m1".into() }]);
    assert_eq!(p(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#, &mut st), vec![Ev::Delta("Hel".into())]);
    assert_eq!(p(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}}"#, &mut st), vec![Ev::Delta("Hello".into())]);
    assert_eq!(p(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#, &mut st), vec![Ev::Thinking("hm".into())]);
    let evs = p(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"},{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"a"}}]}}"#, &mut st);
    assert_eq!(evs[0], Ev::Text("Hello".into()));
    assert!(matches!(&evs[1], Ev::ToolUse { id, name, .. } if id == "t1" && name == "Edit"));
    assert!(st.partial.is_empty(), "the partial text is replaced by the whole message");
    // the same tool_use again (a second message carrying it) is not announced twice
    assert!(p(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{}}]}}"#, &mut st).is_empty());
    assert_eq!(p(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"}],"is_error":true}]}}"#, &mut st), vec![Ev::ToolResult { id: "t1".into(), ok: false, text: "ok".into() }]);
    let done = p(r#"{"type":"result","subtype":"success","is_error":false,"result":"fin","session_id":"s1","total_cost_usd":0.5,"usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":3}}"#, &mut st);
    assert_eq!(done, vec![Ev::Done { ok: true, text: "fin".into(), session: Some("s1".into()), cost_usd: Some(0.5), input: 15, output: 3, cached: 5, error: None }]);
    // a line that is not JSON is a note, never a crash
    assert!(matches!(p("garbage {", &mut st).as_slice(), [Ev::Note(_)]));
    assert_eq!(st.bad_lines, 1);
    let err = p(r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#, &mut st);
    assert!(matches!(&err[0], Ev::Done { ok: false, error: Some(e), .. } if e == "error_max_turns"));
}

#[test]
fn a_claude_run_streams_text_thinking_and_tool_blocks_and_records_the_vendors_numbers() {
    let rig = Rig::new("run");
    let h = start(rig.request(rig.claude(), "list the files"));
    let ev = collect(&h);
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Delta { text } if text == "Looking now.")), "{ev:?}");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Thinking { text, .. } if text == "plan it")));
    // the text that came before the tool call travels with it, like the built-in assistant's turn
    let model = ev.iter().find_map(|e| if let RunEvent::Model { text, calls } = e { Some((text.clone(), calls.clone())) } else { None }).expect("a tool turn");
    assert_eq!(model.0, "Looking now.");
    assert_eq!((model.1[0].id.as_str(), model.1[0].name.as_str()), ("toolu_1", "Bash"));
    assert!(ev.iter().any(|e| matches!(e, RunEvent::ToolResult { id, name, ok: true, text } if id == "toolu_1" && name == "Bash" && text == "a.txt")));
    // IYAKKI's own tools are shown under their own names
    assert!(ev.iter().any(|e| matches!(e, RunEvent::ToolResult { name, text, .. } if name == "board_list" && text == "B-1 open")), "{ev:?}");
    let usage = ev.iter().find_map(|e| if let RunEvent::Usage { tokens, cost_cents, .. } = e { Some((*tokens, *cost_cents)) } else { None }).unwrap();
    assert_eq!(usage.0, 160, "100 + 20 new input and 40 output; the 30 cache-read tokens are not counted again");
    assert!((usage.1 - 1.23).abs() < 1e-9, "{usage:?}");
    assert!(matches!(ev.last().unwrap(), RunEvent::Final { text } if text == "Done: found a.txt"));
    assert_eq!(*rig.usage.lock().unwrap(), vec![("Claude Code".to_string(), "claude-sonnet-x".to_string(), 150, 40)]);
    assert_eq!(*rig.sessions.lock().unwrap().first().unwrap(), "sess-1");
    let log = rig.audit.lock().unwrap().join("\n");
    assert!(log.contains("extagent.start Claude Code"), "{log}");
}

#[test]
fn the_next_message_resumes_the_vendor_session_and_the_flags_hold_the_agent_to_the_workspace() {
    let rig = Rig::new("resume");
    let mut r = rig.request(rig.claude(), "continue please");
    r.resume = Some("sess-1".into());
    r.model = "sonnet".into();
    collect(&start(r));
    let a = rig.args();
    let has = |k: &str, v: &str| a.windows(2).any(|w| w[0] == k && w[1] == v);
    assert!(has("--resume", "sess-1") && has("--model", "sonnet"), "{a:?}");
    for f in ["-p", "--verbose", "--include-partial-messages", "--strict-mcp-config"] {
        assert!(a.iter().any(|x| x == f), "{f} in {a:?}");
    }
    assert!(has("--output-format", "stream-json") && has("--permission-prompt-tool", "mcp__iyakki__approve") && has("--permission-mode", "default"), "{a:?}");
    assert!(a.iter().any(|x| x == "--allowedTools=mcp__iyakki__*"));
    let dis = a.iter().find(|x| x.starts_with("--disallowedTools=")).expect("raw cluster commands are denied");
    for p in ["Bash(kubectl *)", "Bash(k3s *)", "Bash(kubeadm *)", "Bash(docker rm *)", "Bash(sudo *)"] {
        assert!(dis.contains(p), "{p} in {dis}");
    }
    // only the workspace is added; the working folder is the workspace
    let adds: Vec<&String> = a.windows(2).filter(|w| w[0] == "--add-dir").map(|w| &w[1]).collect();
    assert_eq!(adds, vec![&rig.ws.display().to_string()]);
    assert_eq!(std::fs::read_to_string(rig.dir.join("cwd.txt")).unwrap().trim(), rig.ws.display().to_string());
    // the PROJECT STATE and the rules (board ids, do it yourself, history) reach the agent as the system prompt
    let sys = &a[a.iter().position(|x| x == "--append-system-prompt").unwrap() + 1..].join("\n");
    assert!(sys.contains("PROJECT STATE") && sys.contains("B-<n>") && sys.contains("Do the work yourself") && sys.contains("history_search"), "{sys}");
    // the MCP config is a file in a private folder and says to start IYAKKI with --mcp-tools
    let cfg_path = &a[a.iter().position(|x| x == "--mcp-config").unwrap() + 1];
    assert!(cfg_path.contains("iyakki-ext-"), "{cfg_path}");
    assert!(!Path::new(cfg_path).exists(), "the socket folder is removed when the run ends");
    // the request is the last argument, after `--`
    assert_eq!(a.last().unwrap(), "continue please");
    assert_eq!(a[a.len() - 2], "--");
}

#[test]
fn an_api_key_travels_only_in_the_environment_and_nothing_inherited_leaks() {
    let rig = Rig::new("key");
    std::env::set_var("GH_TOKEN", "host-gh-token-should-not-pass");
    let mut r = rig.request(rig.claude(), "use the key");
    r.key = Some(Zeroizing::new("sk-ant-TESTSECRET-123".into()));
    let ev = collect(&start(r));
    std::env::remove_var("GH_TOKEN");
    let env = std::fs::read_to_string(rig.dir.join("env.txt")).unwrap();
    assert!(env.contains("ANTHROPIC_API_KEY=sk-ant-TESTSECRET-123"), "the child gets the key");
    assert!(!env.contains("host-gh-token"), "the person's other tokens are not inherited");
    assert!(!rig.args().join(" ").contains("TESTSECRET"), "never in the arguments");
    assert!(!format!("{ev:?}").contains("TESTSECRET"), "never in the events");
    assert!(!rig.audit.lock().unwrap().join("\n").contains("TESTSECRET"), "never in the audit");
}

#[test]
fn stop_kills_the_whole_process_group() {
    let rig = Rig::new("cancel");
    let h = start(rig.request(rig.claude(), "HANG forever"));
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !rig.dir.join("child.pid").exists() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    let leader: u32 = std::fs::read_to_string(rig.dir.join("leader.pid")).unwrap().trim().parse().unwrap();
    let child: u32 = std::fs::read_to_string(rig.dir.join("child.pid")).unwrap().trim().parse().unwrap();
    assert!(proc::group_alive(leader), "the group is running");
    h.cancel();
    let ev = collect(&h);
    assert!(matches!(ev.last().unwrap(), RunEvent::Stopped { .. }), "{ev:?}");
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while proc::group_alive(leader) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!proc::group_alive(leader), "the group is gone");
    // the sleep the script started is not left behind (not a zombie either: the group was reaped)
    assert!(!Path::new(&format!("/proc/{child}")).exists() || std::fs::read_to_string(format!("/proc/{child}/stat")).map_or(true, |s| s.contains(") Z ") || s.contains(") X ")), "child {child} still running");
}

#[test]
fn a_timeout_stops_the_agent_with_a_plain_message() {
    let rig = Rig::new("timeout");
    let mut r = rig.request(rig.claude(), "HANG");
    r.limits.max_seconds = 1;
    let ev = collect(&start(r));
    assert!(failed(&ev).contains("time limit"), "{ev:?}");
}

// ------------------------------------------------------------------------------------------------ failures

#[test]
fn failures_are_explained_with_the_fix_never_silent() {
    let rig = Rig::new("fail");
    let f = |word: &str| failed(&collect(&start(rig.request(rig.claude(), word))));
    let m = f("AUTHFAIL");
    assert!(m.contains("not signed in") && m.contains("auth login"), "{m}");
    assert!(f("RATE").contains("rate limited"));
    assert!(f("NETDOWN").contains("network"));
    let m = f("EMPTY");
    assert!(m.contains("without an answer"), "{m}");
    let m = f("RESULTERR");
    assert!(m.contains("not signed in"), "{m}");
    // not installed: the exact install command, no empty chat
    let gone: Arc<dyn ExternalAgent> = Arc::new(ClaudeCode::new(AgentCfg { command: "/nonexistent/claude-xyz".into(), ..Default::default() }));
    let m = failed(&collect(&start(rig.request(gone, "hello"))));
    assert!(m.contains("not installed") && m.contains("npm install -g @anthropic-ai/claude-code"), "{m}");
}

#[test]
fn bytes_that_are_not_utf8_and_lines_that_are_not_json_do_not_stop_the_run() {
    let rig = Rig::new("bytes");
    let ev = collect(&start(rig.request(rig.claude(), "BADBYTES")));
    assert!(matches!(ev.last().unwrap(), RunEvent::Final { text } if text == "Done: found a.txt"), "{ev:?}");
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("not JSON"))));
}

// ------------------------------------------------------------------------------------------------ the other agents

#[test]
fn copilot_set_to_advisor_only_has_no_write_or_shell_and_reports_what_it_changed_anyway() {
    let rig = Rig::new("copilot");
    let a: Arc<dyn ExternalAgent> = Arc::new(Copilot::new(AgentCfg { command: copilot(&rig.dir).display().to_string(), advisor_only: true, ..Default::default() }));
    let ev = collect(&start(rig.request(a.clone(), "how do I count words")));
    assert!(matches!(ev.last().unwrap(), RunEvent::Final { text } if text == "Use a HashMap."), "{ev:?}");
    let a_ = rig.args();
    for f in ["--output-format=json", "--no-ask-user", "--deny-tool=write", "--deny-tool=shell", "-p"] {
        assert!(a_.iter().any(|x| x == f), "{f} in {a_:?}");
    }
    assert!(a_.iter().all(|x| !x.contains("allow-all") && !x.contains("yolo") && !x.contains("--additional-mcp-config") && !x.contains("--allow-tool")), "{a_:?}");
    assert!(a_.iter().any(|x| x == "--available-tools=view,glob,grep"), "only the read tools: {a_:?}");
    assert!(a_.iter().any(|x| x == &format!("--add-dir={}", rig.ws.display())));
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("changed files"))));
    // an agent that writes anyway is reported, not hidden
    let ev = collect(&start(rig.request(a, "SNEAKY please")));
    let note = ev.iter().find_map(|e| if let RunEvent::Note { text } = e { Some(text.clone()) } else { None }).expect("a note about the file");
    assert!(note.contains("without IYAKKI's approval") && note.contains("changed sneaky.txt"), "{note}");
}

#[test]
fn a_custom_text_command_gets_the_request_on_stdin_or_in_place_of_the_placeholder() {
    let rig = Rig::new("generic");
    let path = texty(&rig.dir).display().to_string();
    let by_stdin: Arc<dyn ExternalAgent> = Arc::new(Generic::new(CustomCfg { id: "t".into(), name: "Texty".into(), command: format!("{path} --stdin"), key_env: "TEXTY_KEY".into(), ..Default::default() }));
    let mut r = rig.request(by_stdin, "explain this");
    r.key = Some(Zeroizing::new("custom-secret".into()));
    let ev = collect(&start(r));
    assert!(matches!(ev.last().unwrap(), RunEvent::Final { text } if text.trim() == "first line\nsecond line"), "{ev:?}");
    assert!(std::fs::read_to_string(rig.dir.join("stdin.txt")).unwrap().contains("explain this"));
    assert!(!rig.args().join(" ").contains("custom-secret"));
    let by_arg: Arc<dyn ExternalAgent> = Arc::new(Generic::new(CustomCfg { id: "t".into(), command: format!("{path} --ask \"{{prompt}}\""), ..Default::default() }));
    collect(&start(rig.request(by_arg, "what is rust")));
    let a = rig.args();
    assert_eq!(a[0], "--ask");
    let rest = a[1..].join("\n");
    assert!(rest.contains("what is rust") && rest.contains("PROJECT STATE"), "{a:?}");
}

#[test]
fn vendor_session_ids_are_kept_per_chat_and_agent() {
    let dir = tmp("sessions");
    let m = SessionMap::open(&dir.join("ext-sessions.json"));
    assert_eq!(m.get("chat1", "claude-code"), None);
    m.set("chat1", "claude-code", "abc");
    m.set("chat1", "copilot", "def");
    m.set("chat2", "claude-code", "ghi");
    assert_eq!(m.get("chat1", "claude-code").as_deref(), Some("abc"));
    assert_eq!(SessionMap::open(&dir.join("ext-sessions.json")).get("chat2", "claude-code").as_deref(), Some("ghi"));
    m.forget_chat("chat1");
    assert_eq!((m.get("chat1", "claude-code"), m.get("chat1", "copilot")), (None, None));
    assert_eq!(m.get("chat2", "claude-code").as_deref(), Some("ghi"));
}

// ------------------------------------------------------------------------------------------------ the policy bridge

struct Bench {
    gate: Arc<Gate>,
    rx: Receiver<RunEvent>,
    dtx: Sender<(String, Verdict)>,
    audit: Log,
    ws: PathBuf,
    stub: Arc<Stub>,
}

fn bench(auto: bool) -> Bench {
    let rig = Rig::new("gate");
    let (etx, rx) = channel();
    let (dtx, drx) = channel();
    let log: Log = Default::default();
    let l2 = log.clone();
    let gate = Arc::new(Gate::new(rig.stub.clone(), auto, etx, Decisions::new(drx), Arc::new(Correlator::default()), Arc::new(AtomicBool::new(false)), Duration::from_secs(5), "Claude Code", Arc::new(move |k, d| l2.lock().unwrap().push(format!("{k} {d}"))), &rig.ws, 12_000));
    Bench { gate, rx, dtx, audit: log, ws: rig.ws.clone(), stub: rig.stub.clone() }
}

fn ask_in_thread(g: &Arc<Gate>, input: Value) -> std::thread::JoinHandle<Value> {
    let g = g.clone();
    std::thread::spawn(move || g.approve(&input))
}

fn next_approval(b: &Bench) -> (String, String, String) {
    loop {
        match b.rx.recv_timeout(Duration::from_secs(5)).expect("an approval card") {
            RunEvent::Approval { id, summary, detail, .. } => return (id, summary, detail),
            _ => continue,
        }
    }
}

#[test]
fn a_permission_prompt_becomes_an_approval_card_and_the_answer_reaches_the_cli() {
    let b = bench(false);
    let file = b.ws.join("src.txt").display().to_string();
    let h = ask_in_thread(&b.gate, json!({"tool_name": "Write", "tool_use_id": "toolu_9", "input": {"file_path": file, "content": "hello"}}));
    let (id, summary, detail) = next_approval(&b);
    assert_eq!(id, "toolu_9", "the card carries the tool_use id so it sits under the tool block");
    assert!(summary.contains("Claude Code wants to change src.txt") && detail.contains("hello"), "{summary} / {detail}");
    b.dtx.send((id, Verdict::Approve)).unwrap();
    let r = h.join().unwrap();
    assert_eq!(r["behavior"], "allow");
    assert_eq!(r["updatedInput"]["content"], "hello");
    // denied
    let h = ask_in_thread(&b.gate, json!({"tool_name": "Write", "tool_use_id": "toolu_10", "input": {"file_path": "x.txt", "content": "c"}}));
    let (id, ..) = next_approval(&b);
    b.dtx.send((id, Verdict::Deny)).unwrap();
    let r = h.join().unwrap();
    assert_eq!(r["behavior"], "deny");
    assert!(r["message"].as_str().unwrap().contains("Do not retry"));
    let log = b.audit.lock().unwrap().join("\n");
    assert!(log.contains("Claude Code: Write approved by user") && log.contains("Claude Code: Write denied by user"), "{log}");
}

#[test]
fn edits_are_confined_to_the_workspace_and_iyakki_internals_are_off_limits() {
    let b = bench(false);
    let deny = |tool: &str, input: Value| -> String {
        let r = b.gate.approve(&json!({"tool_name": tool, "tool_use_id": "t", "input": input}));
        assert_eq!(r["behavior"], "deny", "{r}");
        r["message"].as_str().unwrap().to_string()
    };
    assert!(deny("Write", json!({"file_path": "/etc/passwd", "content": "x"})).contains("outside the workspace"));
    assert!(deny("Edit", json!({"file_path": "../escape.txt", "old_string": "a", "new_string": "b"})).contains("outside the workspace"));
    assert!(deny("Write", json!({"file_path": ".iyakki/features.json", "content": "x"})).contains("board_*"));
    assert!(deny("Write", json!({"file_path": ".git/config", "content": "x"})).contains(".git"));
    assert!(deny("Read", json!({"file_path": "/home/someone/.ssh/id_rsa"})).contains("outside the workspace"));
    assert!(deny("Read", json!({"file_path": ".iyakki/secrets.json"})).contains(".iyakki"));
    // a symbolic link that points out of the workspace does not get around it
    let out = tmp("outside");
    std::os::unix::fs::symlink(&out, b.ws.join("link")).unwrap();
    assert!(deny("Write", json!({"file_path": "link/evil.txt", "content": "x"})).contains("outside the workspace"));
    // reading bug reports and evidence is allowed without a card
    for p in [".iyakki/bugs/B-1.json", ".iyakki/evidence/shot.png", "README.md"] {
        assert_eq!(b.gate.approve(&json!({"tool_name": "Read", "tool_use_id": "t", "input": {"file_path": p}}))["behavior"], "allow", "{p}");
    }
    assert!(deny("AskUserQuestion", json!({})).contains("chat"));
    assert!(inside(&b.ws, Path::new("a/../b.txt")).is_some() && inside(&b.ws, Path::new("../x")).is_none());
}

#[test]
fn raw_cluster_and_destructive_shell_commands_are_refused_and_the_rest_asks() {
    for cmd in ["kubectl get pods", "sudo apt install x", "cd x && kubectl delete ns y", "echo hi; k3s server", "FOO=1 kubeadm reset", "docker rm -f web", "podman system prune -a", "/usr/local/bin/kubectl apply -f x", "helm install a b", "x | env kubectl get nodes"] {
        assert!(shell_refusal(cmd).is_some(), "{cmd} must be refused");
    }
    for cmd in ["ls -la", "cargo test -p iyakki-agent", "git status", "docker ps", "echo kubectl"] {
        assert!(shell_refusal(cmd).is_none(), "{cmd} may be asked about");
    }
    assert!(shell_refusal("kubectl get pods").unwrap().contains("k8s_"), "the refusal names the tools to use instead");
    let b = bench(false);
    let r = b.gate.approve(&json!({"tool_name": "Bash", "tool_use_id": "b1", "input": {"command": "kubectl delete ns prod"}}));
    assert_eq!(r["behavior"], "deny");
    assert!(b.rx.try_recv().is_ok_and(|e| matches!(e, RunEvent::Decision { approved: false, ref by, .. } if by == "policy")), "no card is shown for a policy refusal");
    let dis = ClaudeCode::disallowed();
    assert!(dis.contains("Bash(kubectl *)") && dis.contains("Bash(docker rm *)"));
    assert!(matches!(classify("Bash", &json!({"command": "ls"}), Path::new("/w")), Class::Ask { hard: false, .. }));
    assert!(matches!(classify("Bash", &json!({"command": "rm -rf build"}), Path::new("/w")), Class::Ask { hard: true, .. }));
    assert!(matches!(classify("Bash", &json!({"command": "cat /etc/hosts"}), Path::new("/w")), Class::Ask { hard: true, .. }));
}

#[test]
fn auto_answers_the_agents_prompts_except_what_hard_ask_names() {
    let b = bench(true);
    // an edit in the workspace runs without a card
    let r = b.gate.approve(&json!({"tool_name": "Edit", "tool_use_id": "e1", "input": {"file_path": "a.rs", "old_string": "a", "new_string": "b"}}));
    assert_eq!(r["behavior"], "allow");
    let r = b.gate.approve(&json!({"tool_name": "Bash", "tool_use_id": "e2", "input": {"command": "cargo test"}}));
    assert_eq!(r["behavior"], "allow");
    assert!(b.audit.lock().unwrap().iter().any(|l| l.contains("Claude Code: Bash approved by auto-approved")));
    // a destructive command still asks
    let h = ask_in_thread(&b.gate, json!({"tool_name": "Bash", "tool_use_id": "e3", "input": {"command": "rm -rf build"}}));
    let (id, summary, _) = next_approval(&b);
    assert!(summary.contains("rm -rf build"));
    b.dtx.send((id, Verdict::Deny)).unwrap();
    assert_eq!(h.join().unwrap()["behavior"], "deny");
    // and an unknown tool of the agent asks too
    let h = ask_in_thread(&b.gate, json!({"tool_name": "SomethingNew", "tool_use_id": "e4", "input": {}}));
    let (id, ..) = next_approval(&b);
    b.dtx.send((id, Verdict::Approve)).unwrap();
    assert_eq!(h.join().unwrap()["behavior"], "allow");
}

fn rpc(conn: &mut UnixStream, rd: &mut BufReader<UnixStream>, id: u64, method: &str, params: Value) -> Value {
    writeln!(conn, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
    let mut line = String::new();
    rd.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn an_mcp_tool_call_goes_through_the_policy_the_approval_and_the_audit_log() {
    let b = bench(false);
    let bridge = Bridge::start(b.gate.clone(), Path::new("/opt/iyakki/iyakki")).unwrap();
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&bridge.config).unwrap()).unwrap();
    let srv = &cfg["mcpServers"]["iyakki"];
    assert_eq!(srv["command"], "/opt/iyakki/iyakki");
    assert_eq!(srv["args"][0], "--mcp-tools");
    let token = srv["env"]["IYAKKI_MCP_TOKEN"].as_str().unwrap().to_string();
    assert!(!srv["args"].to_string().contains(&token), "the token is in the environment, not the arguments");
    // a connection without the token is dropped
    let mut bad = UnixStream::connect(&bridge.sock).unwrap();
    writeln!(bad, "wrong").unwrap();
    writeln!(bad, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
    bad.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    assert_eq!(BufReader::new(bad).lines().next().map(|l| l.is_ok_and(|s| !s.is_empty())), None, "no answer for a stranger");
    // the real thing
    let mut conn = UnixStream::connect(&bridge.sock).unwrap();
    writeln!(conn, "{token}").unwrap();
    let mut rd = BufReader::new(conn.try_clone().unwrap());
    let init = rpc(&mut conn, &mut rd, 1, "initialize", json!({"protocolVersion": "2025-03-26"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "iyakki");
    let list = rpc(&mut conn, &mut rd, 2, "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in ["board_list", "board_add", "k8s_delete", "run_command", "approve"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    for n in ["vault_request", "write_file", "browser_open"] {
        assert!(!names.contains(&n), "{n} is not offered: {names:?}");
    }
    // safe: runs at once
    let r = rpc(&mut conn, &mut rd, 3, "tools/call", json!({"name": "board_list", "arguments": {}}));
    assert_eq!(r["result"]["content"][0]["text"], "board_list ran");
    // a policy refusal is the tool's answer, nobody is asked
    let r = rpc(&mut conn, &mut rd, 4, "tools/call", json!({"name": "k8s_apply", "arguments": {}}));
    assert_eq!((r["result"]["isError"].as_bool(), r["result"]["content"][0]["text"].as_str()), (Some(true), Some("production is read-only")));
    assert!(!b.stub.ran.lock().unwrap().iter().any(|c| c.starts_with("k8s_apply")));
    // asks: a card, then the call runs only after the person says yes
    writeln!(conn, "{}", json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "board_add", "arguments": {"title": "x"}, "_meta": {"claudecode/toolUseId": "toolu_77"}}})).unwrap();
    let (id, ..) = next_approval(&b);
    assert_eq!(id, "toolu_77");
    assert!(!b.stub.ran.lock().unwrap().iter().any(|c| c.starts_with("board_add")), "not run before the answer");
    b.dtx.send((id, Verdict::Approve)).unwrap();
    let mut line = String::new();
    rd.read_line(&mut line).unwrap();
    let r: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(r["result"]["content"][0]["text"], "board_add ran");
    // hard_ask: a destructive tool asks and a denial is reported as an error
    writeln!(conn, "{}", json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "k8s_delete", "arguments": {}}})).unwrap();
    let (id, ..) = next_approval(&b);
    b.dtx.send((id, Verdict::Deny)).unwrap();
    line.clear();
    rd.read_line(&mut line).unwrap();
    let r: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(!b.stub.ran.lock().unwrap().iter().any(|c| c.starts_with("k8s_delete")));
    // and the agent's own prompt tool is on the same socket
    let r = rpc(&mut conn, &mut rd, 7, "tools/call", json!({"name": "approve", "arguments": {"tool_name": "Bash", "input": {"command": "kubectl get pods"}, "tool_use_id": "t"}}));
    let verdict: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(verdict["behavior"], "deny");
    let log = b.audit.lock().unwrap().join("\n");
    assert!(log.contains("extagent.tool Claude Code: board_list ok") && log.contains("extagent.decision Claude Code: board_add approved by user") && log.contains("extagent.decision Claude Code: k8s_delete denied by user") && log.contains("extagent.refused Claude Code: Bash"), "{log}");
    let dir = bridge.sock.parent().unwrap().to_path_buf();
    drop(bridge);
    assert!(!dir.exists(), "the socket folder is removed");
}

#[test]
fn auto_is_off_for_an_agent_until_the_person_allows_it() {
    let rig = Rig::new("autooff");
    let mut r = rig.request(rig.claude(), "list");
    r.limits.auto = true; // the chat is in Auto, this agent is not allowed to use it
    let ev = collect(&start(r));
    assert!(ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("Auto is off for Claude Code"))), "{ev:?}");
    let mut r = rig.request(rig.claude(), "list");
    r.limits.auto = true;
    r.allow_auto = true;
    let ev = collect(&start(r));
    assert!(!ev.iter().any(|e| matches!(e, RunEvent::Note { text } if text.contains("Auto is off"))));
}

// ------------------------------------------------------------------------------------------------ models

#[test]
fn each_agent_ships_its_documented_models_labelled_as_the_built_in_list() {
    let c = ClaudeCode::new(AgentCfg::default());
    let ids: Vec<String> = c.builtin_models().into_iter().map(|m| m.id).collect();
    assert_eq!(&ids[..4], ["", "sonnet", "opus", "haiku"], "Default, Sonnet, Opus, Haiku come first: {ids:?}");
    let all = c.builtin_models();
    assert!(all.iter().all(|m| m.source == ModelSource::BuiltIn && m.available.is_none()), "nothing is guessed about a plan");
    assert!(all[0].label.starts_with("Default") && all[1].note.contains("daily coding"), "{all:?}");
    assert_eq!(ModelSource::BuiltIn.label(), "built-in list");
    assert!(BUILTIN_VERIFIED.starts_with("2026-"), "the date the lists were checked");
    // Copilot documents no ids for --model: only the default is shipped, the rest is typed
    let p = Copilot::new(AgentCfg::default()).builtin_models();
    assert_eq!(p.len(), 1);
    assert!(p[0].id.is_empty() && p[0].note.contains("/model"), "{p:?}");
    // neither CLI can list its models without a session (documented), so no command is run for them
    assert!(c.list_models_argv().is_none() && Copilot::new(AgentCfg::default()).list_models_argv().is_none());
}

#[test]
fn the_models_offered_are_the_cli_list_when_it_printed_one_else_the_built_in_list_plus_the_ids_typed() {
    let cache = StatusCache::default();
    let cfg = AgentCfg { model: "claude-sonnet-5".into(), custom_models: vec!["my-model:1".into(), "opus".into(), "bad id".into()], ..Default::default() };
    let c = ClaudeCode::new(cfg);
    let v = cache.models_for(&c);
    let ids: Vec<&str> = v.iter().map(|m| m.id.as_str()).collect();
    // built-in, then the typed ids (no duplicate of a listed one, never an invalid one), the chosen one included
    assert!(ids.contains(&"sonnet") && ids.contains(&"my-model:1") && ids.contains(&"claude-sonnet-5") && !ids.contains(&"bad id"), "{ids:?}");
    assert_eq!(ids.iter().filter(|i| **i == "opus").count(), 1);
    assert!(v.iter().find(|m| m.id == "my-model:1").unwrap().source == ModelSource::Custom);
    // a list the CLI printed replaces the built-in one, and goes stale after a day
    cache.set_models("claude-code", vec![ModelInfo::new("only-one", "only-one", "", ModelSource::Cli)]);
    assert!(cache.models_for(&c).iter().any(|m| m.id == "only-one") && !cache.models_for(&c).iter().any(|m| m.id == "haiku"));
    cache.set_models_at("claude-code", vec![ModelInfo::new("old", "old", "", ModelSource::Cli)], now_secs() - MODELS_TTL - 5);
    assert!(cache.cli_models("claude-code").is_none(), "older than 24 h");
    assert!(cache.models_for(&c).iter().any(|m| m.id == "haiku"), "back to the built-in list");
}

#[test]
fn a_model_list_printed_by_a_fake_cli_is_read_after_detect_and_cached_in_a_file() {
    let rig = Rig::new("models");
    let bin = super::fake::modelist(&rig.dir);
    let file = rig.dir.join("ext-models.json");
    let cache = Arc::new(StatusCache::open(&file));
    let cfg = AgentsCfg {
        claude: AgentCfg { command: "/nonexistent/claude-xyz".into(), ..Default::default() },
        copilot: AgentCfg { command: "/nonexistent/copilot-xyz".into(), ..Default::default() },
        custom: vec![CustomCfg { id: "ml".into(), name: "ML".into(), command: format!("{} run", bin.display()), models_command: format!("{} --models", bin.display()), ..Default::default() }],
        ..Default::default()
    };
    assert!(cache.cli_models("custom:ml").is_none(), "nothing runs before Detect");
    cache.refresh(cfg.clone());
    for _ in 0..500 {
        if !cache.busy() && cache.get("custom:ml").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let list = cache.cli_models("custom:ml").expect("the CLI's list");
    let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["fast-1", "big-2", "max-3"], "invalid ids, comments and repeats are dropped: {ids:?}");
    assert_eq!((list[0].note.as_str(), list[1].note.as_str()), ("quick and cheap", "the most capable"));
    assert!(list[2].available == Some(false) && list[0].available.is_none(), "unavailable only because the CLI said so: {list:?}");
    assert!(list.iter().all(|m| m.source == ModelSource::Cli));
    // the CLIs that cannot list were not asked, and the file keeps the list for the next start
    assert!(cache.cli_models("claude-code").is_none());
    let again = StatusCache::open(&file);
    assert_eq!(again.cli_models("custom:ml").unwrap().len(), 3);
    let agent = cfg.agent("custom:ml").unwrap();
    assert_eq!(again.models_for(agent.as_ref()).len(), 3);
}

#[test]
fn a_custom_model_id_with_shell_characters_is_refused_everywhere() {
    for ok in ["sonnet", "opus", "claude-sonnet-5", "sonnet[1m]", "gpt-5.1:high", "vendor/model@2", "a_b+c"] {
        assert!(valid_model_id(ok), "{ok}");
    }
    for bad in ["", " ", "a b", "a;b", "a&b", "a|b", "$(id)", "`id`", "a\"b", "a'b", "a>b", "a<b", "--flag", "-x", "a\nb", "a*b", "a?b", "a\\b", "a(b)", "a{b}", &"x".repeat(81)] {
        assert!(!valid_model_id(bad), "{bad:?}");
    }
    let mut a = AgentsCfg::default();
    a.claude.model = "x; rm -rf /".into();
    assert!(a.validate().unwrap_err().contains("model id"));
    a.claude.model = String::new();
    a.copilot.custom_models.push("a b".into());
    assert!(a.validate().is_err());
    a.copilot.custom_models.clear();
    a.custom.push(CustomCfg { id: "x".into(), command: "x".into(), model: "$(id)".into(), ..Default::default() });
    assert!(a.validate().is_err());
}

#[test]
fn the_chosen_model_reaches_the_childs_arguments_and_only_a_valid_one_does() {
    let rig = Rig::new("modelarg");
    let mut r = rig.request(rig.claude(), "use a model");
    r.model = "claude-opus-5-5".into();
    collect(&start(r));
    let a = rig.args();
    assert!(a.windows(2).any(|w| w[0] == "--model" && w[1] == "claude-opus-5-5"), "{a:?}");
    // Default: no flag, even when the agent's own setting names a model (the caller decides)
    let agent: Arc<dyn ExternalAgent> = Arc::new(ClaudeCode::new(AgentCfg { command: rig.dir.join("claude").display().to_string(), model: "sonnet".into(), ..Default::default() }));
    let mut r = rig.request(agent.clone(), "default model");
    r.model = String::new();
    collect(&start(r));
    assert!(!rig.args().iter().any(|x| x == "--model"), "{:?}", rig.args());
    // something that is not a model id never becomes an argument
    let mut r = rig.request(agent, "evil model");
    r.model = "x; touch pwned".into();
    collect(&start(r));
    assert!(!rig.args().iter().any(|x| x.contains("pwned") || x == "--model"), "{:?}", rig.args());
    // Copilot gets --model=<id>
    let c: Arc<dyn ExternalAgent> = Arc::new(Copilot::new(AgentCfg { command: copilot(&rig.dir).display().to_string(), ..Default::default() }));
    let mut r = rig.request(c, "ask");
    r.model = "some-model".into();
    collect(&start(r));
    assert!(rig.args().iter().any(|x| x == "--model=some-model"));
}

#[test]
fn model_lines_are_read_one_per_line_with_an_optional_note() {
    let l = parse_model_lines("a-1\nb-2  fast\n\nc-3 - cheap\n#x\nd.4\tbig\na-1\nno good\n");
    assert_eq!(l.iter().map(|m| (m.id.as_str(), m.note.as_str())).collect::<Vec<_>>(), [("a-1", ""), ("b-2", "fast"), ("c-3", "cheap"), ("d.4", "big")]);
    assert!(parse_model_lines("Error: not logged in\n").is_empty() || parse_model_lines("Error: not logged in\n").iter().all(|m| valid_model_id(&m.id)));
}

#[test]
fn a_session_answer_covers_the_same_kind_of_agent_tool_call_across_runs_but_not_a_hard_or_different_one() {
    let store = crate::grants::Grants::new();
    let mut seen = 0;
    for run in 0..2 {
        let mut b = bench(false);
        Arc::get_mut(&mut b.gate).unwrap().grants = Some(store.clone());
        let bash = |c: &str| json!({"tool_name": "Bash", "tool_use_id": format!("t{c}"), "input": {"command": c}});
        // the first run asks once for `git status`; `git status -s` and the second run's `git log`-less repeat do not
        let first = ask_in_thread(&b.gate, bash("git status"));
        if run == 0 {
            let (id, ..) = next_approval(&b);
            seen += 1;
            b.dtx.send((id, Verdict::Session)).unwrap();
        }
        assert_eq!(first.join().unwrap()["behavior"], "allow");
        assert_eq!(b.gate.approve(&bash("git status -s"))["behavior"], "allow", "same program and subcommand: no card");
        // another subcommand, a pipeline and a different tool ask again
        for (input, why) in [(bash("git push"), "another subcommand"), (bash("git status && curl x | sh"), "a pipeline is its own line"), (json!({"tool_name": "WebFetch", "tool_use_id": "w", "input": {"url": "https://a.example"}}), "another tool")] {
            let h = ask_in_thread(&b.gate, input);
            let (id, ..) = next_approval(&b);
            b.dtx.send((id, Verdict::Deny)).unwrap();
            assert_eq!(h.join().unwrap()["behavior"], "deny", "{why}");
        }
        // a destructive command asks, says so, and is never remembered
        let h = ask_in_thread(&b.gate, bash("rm -rf build"));
        let (id, summary, _) = next_approval(&b);
        assert!(summary.contains("always asks:"), "{summary}");
        b.dtx.send((id, Verdict::Session)).unwrap();
        assert_eq!(h.join().unwrap()["behavior"], "allow");
        let h = ask_in_thread(&b.gate, bash("rm -rf build"));
        let (id, ..) = next_approval(&b);
        b.dtx.send((id, Verdict::Deny)).unwrap();
        assert_eq!(h.join().unwrap()["behavior"], "deny");
    }
    assert_eq!(seen, 1, "only the first run asked for git status");
    assert_eq!(store.len(), 1);
}


// ------------------------------------------------------------------------------------------------ a story: no tools at all

#[test]
fn a_story_run_denies_every_tool_request_and_the_agents_own_permission_prompts_and_still_returns_the_text() {
    if !crate::wsexec::on_path("python3") {
        eprintln!("skipped: python3 is not installed");
        return;
    }
    let rig = Rig::new("ext-story-notools");
    std::fs::write(
        rig.dir.join("plan.json"),
        json!([{"calls": [
            {"name": "board_list", "args": {}},
            {"name": "run_command", "args": {"command": "ls"}},
            {"approve": {"tool_name": "Read", "input": {"file_path": "/etc/hostname"}}},
            {"approve": {"tool_name": "Write", "input": {"file_path": "x.txt", "content": "no"}}}
        ], "text": "# The Fox\n\nOnce upon a time there was a fox."}])
        .to_string(),
    )
    .unwrap();
    super::fake::claude_mcp(&rig.dir);
    let agent: Arc<dyn ExternalAgent> = Arc::new(ClaudeCode::new(AgentCfg { command: rig.dir.join("claude").display().to_string(), ..Default::default() }));
    let mut req = rig.request(agent, "Write a story");
    req.no_tools = true;
    req.system = Arc::new(|| "STORY RULES".to_string());
    let h = start(req);
    let mut seen = String::new();
    let got = crate::story::follow_agent_run(&h, &|| false, &mut |t| seen = t.to_string(), &mut |_| {});
    assert!(got.as_ref().is_ok_and(|t| t.contains("Once upon a time")), "{got:?}");
    assert!(rig.stub.ran.lock().unwrap().is_empty(), "no tool of IYAKKI ran: {:?}", rig.stub.ran.lock().unwrap());
    assert!(!rig.ws.join("x.txt").exists(), "the agent's own Write was denied");
    let audit = rig.audit.lock().unwrap().join("\n");
    assert!(audit.matches("extagent.refused").count() >= 4, "every request was refused and audited:\n{audit}");
    assert!(!audit.contains("extagent.tool"), "{audit}");
    let args = std::fs::read_to_string(rig.dir.join("args1.txt")).unwrap();
    assert!(args.contains("--disallowedTools=") && args.contains("Read,") && args.contains("WebFetch"), "the agent's own tools are switched off too: {args}");
    assert!(args.contains("STORY RULES") && !args.contains("running inside IYAKKI's AI IDE"), "the IDE rules are not added to a story: {args}");
}

// ------------------------------------------------------------------------------------------------ GitHub Copilot through the bridge
// The argv, the config and the event shapes below were checked against a live Copilot CLI 1.0.98 (docs/EXTERNAL-AGENTS.md).

#[test]
fn copilot_through_the_bridge_sees_only_read_tools_and_iyakki_and_may_use_only_that_server() {
    let rig = Rig::new("copilot-bridged");
    let a: Arc<dyn ExternalAgent> = Arc::new(Copilot::new(AgentCfg { command: copilot(&rig.dir).display().to_string(), ..Default::default() }));
    assert_eq!(a.mode(), Mode::Bridged, "the bridge is the default");
    let mut r = rig.request(a.clone(), "list the board");
    r.resume = Some("0fa1b0d1-a217-417e-b894-f22c9bb7304d".into());
    let ev = collect(&start(r));
    assert!(matches!(ev.last().unwrap(), RunEvent::Final { text } if text == "Use a HashMap."), "{ev:?}");
    let a_ = rig.args();
    let cfg = a_.iter().find_map(|x| x.strip_prefix("--additional-mcp-config=@")).expect("the MCP config").to_string();
    assert!(cfg.ends_with("mcp-copilot.json"), "{cfg}");
    for f in ["--disable-builtin-mcps", "--allow-tool=iyakki", "--available-tools=view,glob,grep,iyakki", "--deny-tool=write", "--deny-tool=shell", "--no-ask-user", "--output-format=json", "--resume=0fa1b0d1-a217-417e-b894-f22c9bb7304d"] {
        assert!(a_.iter().any(|x| x == f), "{f} in {a_:?}");
    }
    assert!(a_.iter().all(|x| !x.contains("allow-all") && !x.contains("yolo") && !x.contains("--allow-tool=write") && !x.contains("--allow-tool=shell")), "{a_:?}");
    let prompt = a_.join("\n");
    assert!(prompt.contains("iyakki-write_file") && prompt.contains("iyakki-run_command") && !prompt.contains("mcp__iyakki__") && !prompt.contains("an advisor"), "{prompt}");
    // the agent's tool blocks use IYAKKI's names, and the run's tool calls came out of the verified shapes
    let shown: Vec<String> = ev.iter().filter_map(|e| if let RunEvent::Model { calls, .. } = e { Some(calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(",")) } else { None }).collect();
    assert!(shown.contains(&"board_list".to_string()) && shown.contains(&"bash".to_string()), "{shown:?}");
    let results: Vec<(String, bool, String)> = ev.iter().filter_map(|e| if let RunEvent::ToolResult { name, ok, text, .. } = e { Some((name.clone(), *ok, text.clone())) } else { None }).collect();
    assert!(results.iter().any(|(n, ok, t)| n == "board_list" && *ok && t == "no rows"), "{results:?}");
    assert!(results.iter().any(|(n, ok, t)| n == "bash" && !*ok && t.contains("denied")), "{results:?}");
    // a story run (no tools) leaves the model nothing of its own
    let mut r = rig.request(a, "write a story");
    r.no_tools = true;
    collect(&start(r));
    assert!(rig.args().iter().any(|x| x == "--available-tools=iyakki"), "{:?}", rig.args());
}

#[test]
fn the_copilot_flavour_of_the_bridge_config_has_the_verified_keys_and_a_long_call_timeout() {
    let b = bench(false);
    let bridge = Bridge::start(b.gate.clone(), Path::new("/opt/iyakki/iyakki")).unwrap();
    let path = super::bridge::copilot_config(&bridge.config);
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let srv = &cfg["mcpServers"]["iyakki"];
    assert_eq!((srv["type"].as_str(), srv["command"].as_str(), srv["args"][0].as_str(), srv["tools"][0].as_str()), (Some("stdio"), Some("/opt/iyakki/iyakki"), Some("--mcp-tools"), Some("*")));
    assert!(srv["timeout"].as_u64().unwrap() >= 600_000, "an approval card may wait for the person");
    assert!(srv["env"]["IYAKKI_MCP_TOKEN"].as_str().is_some_and(|t| t.len() == 32) && !srv["args"].to_string().contains(srv["env"]["IYAKKI_MCP_TOKEN"].as_str().unwrap()));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    drop(bridge);
    assert!(!path.exists(), "removed with the run");
}

#[test]
fn copilot_json_lines_are_read_from_the_verified_shapes() {
    let c = Copilot::new(AgentCfg::default());
    let mut st = ParseState::default();
    let mut feed = |l: &str| c.parse_line(l, &mut st);
    assert!(feed(r#"{"type":"session.tools_updated","data":{"model":"m"},"ephemeral":true}"#).is_empty());
    assert!(feed(r#"{"type":"assistant.tool_call_delta","data":{"toolCallId":"t"},"ephemeral":true}"#).is_empty(), "a tool-call delta is not a tool call");
    assert!(matches!(&feed(r#"{"type":"assistant.message_delta","data":{"deltaContent":"PONG-IYAK"}}"#)[..], [Ev::Delta(t)] if t == "PONG-IYAK"));
    assert!(matches!(&feed(r#"{"type":"assistant.message_delta","data":{"deltaContent":"KI"}}"#)[..], [Ev::Delta(t)] if t == "PONG-IYAKKI"));
    assert!(matches!(&feed(r#"{"type":"assistant.message","data":{"content":"PONG-IYAKKI","toolRequests":[]}}"#)[..], [Ev::Text(t)] if t == "PONG-IYAKKI"));
    assert!(feed(r#"{"type":"assistant.message","data":{"content":"","toolRequests":[{"name":"iyakki-ping_ws"}]}}"#).is_empty());
    let start = r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_1","toolName":"iyakki-ping_ws","arguments":{"a":1},"mcpServerName":"iyakki","mcpToolName":"ping_ws"}}"#;
    assert!(matches!(&feed(start)[..], [Ev::ToolUse { id, name, input }] if id == "toolu_1" && name == "ping_ws" && input["a"] == 1));
    assert!(feed(start).is_empty(), "announced once");
    assert!(matches!(&feed(r#"{"type":"tool.execution_start","data":{"toolCallId":"b","toolName":"bash","arguments":{}}}"#)[..], [Ev::ToolUse { name, .. }] if name == "bash"));
    assert!(matches!(&feed(r#"{"type":"tool.execution_complete","data":{"toolCallId":"toolu_1","success":true,"result":{"content":"PONG-IYAKKI"}}}"#)[..], [Ev::ToolResult { ok: true, text, .. }] if text == "PONG-IYAKKI"));
    assert!(matches!(&feed(r#"{"type":"tool.execution_complete","data":{"toolCallId":"b","success":false,"error":{"message":"Permission denied because no interactive user response was available.","code":"denied"}}}"#)[..], [Ev::ToolResult { ok: false, text, .. }] if text.contains("Permission denied")));
    assert!(matches!(&feed(r#"{"type":"result","sessionId":"s-1","exitCode":0,"usage":{"premiumRequests":1}}"#)[..], [Ev::Done { ok: true, session: Some(s), error: None, .. }] if s == "s-1"));
    assert!(matches!(&feed(r#"{"type":"result","sessionId":"s-2","exitCode":1}"#)[..], [Ev::Done { ok: false, error: Some(_), .. }]));
}

#[test]
fn copilot_is_told_its_tool_names_and_gets_write_file_only_through_the_gate_with_an_approval_card() {
    let r = super::run::rules("GitHub Copilot", Mode::Bridged);
    assert!(r.contains("iyakki-write_file") && r.contains("iyakki-k8s_") && r.contains("iyakki-context_add"), "{r}");
    assert!(!r.contains("mcp__iyakki__"), "Copilot names MCP tools <server>-<tool>");
    let c = super::run::rules("Claude Code", Mode::Bridged);
    assert!(c.contains("mcp__iyakki__") && !c.contains("iyakki-write_file"));
    let mut b = bench(false);
    assert!(b.gate.call_tool("write_file", &json!({"path": "a.txt"}), None).unwrap_err().contains("not available"), "Claude Code has its own Write");
    Arc::get_mut(&mut b.gate).expect("the only owner").offer_write_file();
    let g = b.gate.clone();
    let h = std::thread::spawn(move || g.call_tool("write_file", &json!({"path": "a.txt", "content": "x"}), Some("t1".into())));
    let (id, _, _) = next_approval(&b);
    assert!(b.stub.ran.lock().unwrap().is_empty(), "nothing ran before the person decided");
    b.dtx.send((id, Verdict::Deny)).unwrap();
    assert!(h.join().unwrap().unwrap_err().contains("denied"));
    assert!(b.stub.ran.lock().unwrap().is_empty());
    // the raw cluster tools stay refused by policy, not by a prompt
    assert!(b.gate.call_tool("k8s_apply", &json!({}), None).unwrap_err().contains("read-only"));
}

// ------------------------------------------------------------------------------------------------ Copilot's real handshake bytes

/// The lines GitHub Copilot CLI 1.0.98 sent to the server on a real device (recorded from the relay's standard input; no secrets):
/// a `server/discover` first, then `initialize`, the `initialized` notification and `tools/list`.
const COPILOT_HANDSHAKE: [&str; 4] = [
    r#"{"jsonrpc":"2.0","id":0,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"copilot-cli","version":"1.0.98"},"io.modelcontextprotocol/clientCapabilities":{"sampling":{},"elicitation":{"form":{},"url":{}}}}}}"#,
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"sampling":{},"elicitation":{"form":{},"url":{}}},"clientInfo":{"name":"copilot-cli","version":"1.0.98"}}}"#,
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"progressToken":0}}}"#,
];

#[test]
fn copilots_real_handshake_is_answered_line_by_line_without_waiting_for_the_next_line() {
    let b = bench(false);
    let bridge = Bridge::start(b.gate.clone(), Path::new("/opt/iyakki/iyakki")).unwrap();
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&bridge.config).unwrap()).unwrap();
    let token = cfg["mcpServers"]["iyakki"]["env"]["IYAKKI_MCP_TOKEN"].as_str().unwrap().to_string();
    assert_eq!(std::fs::read_to_string(bridge.sock.with_file_name("token")).unwrap().trim(), token, "the relay can find the token without the env block");
    let mut conn = UnixStream::connect(&bridge.sock).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    writeln!(conn, "{token}").unwrap();
    let mut rd = BufReader::new(conn.try_clone().unwrap());
    let mut w = conn.try_clone().unwrap();
    let mut ask = |line: &str| -> Value {
        writeln!(w, "{line}").unwrap();
        let mut out = String::new();
        rd.read_line(&mut out).expect("an answer before the next line is sent");
        serde_json::from_str(&out).unwrap()
    };
    // an unknown first request is answered with an error at once (Copilot waits 10 s for any answer, then falls back)
    let d = ask(COPILOT_HANDSHAKE[0]);
    assert_eq!((d["id"].as_u64(), d["error"]["code"].as_i64()), (Some(0), Some(-32601)), "{d}");
    let i = ask(COPILOT_HANDSHAKE[1]);
    assert_eq!((i["id"].as_u64(), i["result"]["protocolVersion"].as_str(), i["result"]["serverInfo"]["name"].as_str()), (Some(1), Some("2025-11-25"), Some("iyakki")), "{i}");
    let t = ask(&format!("{}\n{}", COPILOT_HANDSHAKE[2], COPILOT_HANDSHAKE[3])); // a notification has no answer
    assert_eq!(t["id"].as_u64(), Some(2));
    assert!(t["result"]["tools"].as_array().unwrap().iter().any(|x| x["name"] == "board_list"), "{t}");
}

#[test]
fn the_relay_passes_each_chunk_on_at_once_in_both_directions() {
    // the kernel fast path of std::io::copy held a message back until the next one came; `pump` must not
    let (mut a_in, a_out) = UnixStream::pair().unwrap();
    let (b_in, mut b_out) = UnixStream::pair().unwrap();
    let h = std::thread::spawn(move || {
        let mut from = a_out;
        super::bridge::pump(&mut from, &mut b_out);
    });
    a_in.write_all(b"first line\n").unwrap();
    let mut rd = BufReader::new(b_in);
    rd.get_ref().set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut got = String::new();
    rd.read_line(&mut got).expect("the first line arrives without a second one");
    assert_eq!(got, "first line\n");
    drop(a_in);
    h.join().unwrap();
}

#[test]
fn the_relay_finds_the_token_in_a_file_when_the_env_block_is_not_passed() {
    if std::env::var("IYAKKI_MCP_TOKEN").is_ok() {
        return;
    }
    let d = tmp("relay-token");
    assert_eq!(super::bridge::relay_token(&d.join("mcp.sock").display().to_string()), None);
    std::fs::write(d.join("token"), "abc123\n").unwrap();
    assert_eq!(super::bridge::relay_token(&d.join("mcp.sock").display().to_string()).as_deref(), Some("abc123"));
}

// ------------------------------------------------------------------------------------------------ "an IYAKKI app" means a tile app

#[test]
fn an_iyakki_app_request_puts_the_tile_authoring_guidance_in_front_of_claude_and_copilot_and_other_requests_get_none() {
    for (name, mk) in [("claude", 0), ("copilot", 1)] {
        let rig = Rig::new(&format!("guide-{name}"));
        let a: Arc<dyn ExternalAgent> = if mk == 0 {
            Arc::new(ClaudeCode::new(AgentCfg { command: claude(&rig.dir).display().to_string(), ..Default::default() }))
        } else {
            Arc::new(Copilot::new(AgentCfg { command: copilot(&rig.dir).display().to_string(), ..Default::default() }))
        };
        for p in ["develop an IYAKKI app that adds two numbers and deploy it to the private store", "build a tile for the launcher", "publish it to the private store"] {
            collect(&start(rig.request(a.clone(), p)));
            let all = rig.args().join("\n");
            assert!(all.contains("THE PERSON ASKED FOR AN IYAKKI APP") && all.contains("Do NOT use Rust, Python, Node servers, Docker") && all.contains("--- skill iyakki-tile-author ---") && all.contains("tile_scaffold") && all.contains("tile_ship") && all.contains("Never ask the person for keys, accounts, tokens or sign-in"), "{name} {p}: {all}");
            assert!(all.contains("IYAKKI is an operating system for agents"), "the rules say what IYAKKI is");
        }
        for p in ["fix the failing parser test", "add a clear button to the python script", "deploy the api to k3s with a Dockerfile", "write a story about a fox"] {
            collect(&start(rig.request(a.clone(), p)));
            let all = rig.args().join("\n");
            assert!(!all.contains("THE PERSON ASKED FOR AN IYAKKI APP") && !all.contains("--- skill"), "{name} {p}");
            assert!(all.contains("never a Python, Node or Docker web service on the cluster unless they explicitly ask"), "the general rule is always there");
        }
    }
}

#[test]
fn guidance_picks_the_skills_by_what_is_asked_and_stays_small() {
    use super::guidance::{for_prompt, skills_for};
    assert_eq!(skills_for("make an iyakki app"), vec!["iyakki-app-author", "iyakki-tile-author"]);
    assert_eq!(skills_for("create an MCP server for iyakki"), vec!["iyakki-app-author", "iyakki-mcp-author"]);
    assert_eq!(skills_for("write an iyakki skill for meeting notes"), vec!["iyakki-app-author", "iyakki-skill-author"]);
    assert!(skills_for("an iyakki app on k3s with docker and a cluster").is_empty(), "asked for both: the person is asked, not told");
    assert!(skills_for("refactor the module").is_empty());
    let g = for_prompt("an iyakki app");
    assert!(g.len() < 16_000 && !g.contains("triggers:"), "front matter is left out ({} bytes)", g.len());
}

#[test]
fn the_bridge_offers_the_tools_to_make_and_publish_a_tile_app_and_not_the_ones_that_install_or_remove_apps() {
    use crate::agent::{Risk, ToolDef};
    let d = |n: &str, r: Risk| ToolDef { spec: ToolSpec { name: n.into(), description: String::new(), schema: json!({"type": "object"}) }, risk: r, untrusted: false };
    for n in ["tile_scaffold", "tile_test", "tile_validate", "tile_pack", "tile_install_dev", "app_preflight", "app_scaffold", "app_bump_version", "store_validate", "store_publish", "store_server_status", "publisher_keys"] {
        assert!(super::bridge::offered(&d(n, Risk::Ask)), "{n}");
    }
    for n in ["store_install", "store_remove", "store_update", "store_rollback"] {
        assert!(!super::bridge::offered(&d(n, Risk::Ask)), "{n}");
    }
}

// ------------------------------------------------------------------------------------------------ the live mode and an external agent

#[test]
fn switching_the_mode_during_an_external_run_applies_to_the_waiting_card_and_the_next_calls() {
    use crate::livemode::{LiveMode, Mode as Live};
    // Autopilot on, but Auto not allowed for this agent: every action still asks, and the note says why (once)
    let mut b = bench(false);
    let live = LiveMode::new(Live::Autopilot);
    {
        let g = Arc::get_mut(&mut b.gate).unwrap();
        g.live = Some(live.clone());
        g.allow_auto = false;
    }
    let g = b.gate.clone();
    let h = std::thread::spawn(move || g.call_tool("board_add", &json!({"title": "x"}), Some("c1".into())));
    let mut notes = vec![];
    let id = loop {
        match b.rx.recv_timeout(Duration::from_secs(5)).expect("an approval card") {
            RunEvent::Approval { id, .. } => break id,
            RunEvent::Note { text } => notes.push(text),
            _ => {}
        }
    };
    assert!(notes.iter().any(|n| n.contains("Autopilot is on, but Auto is off for Claude Code") && n.contains("Settings → Agents")), "{notes:?}");
    b.dtx.send((id, Verdict::Approve)).unwrap();
    assert!(h.join().unwrap().is_ok());
    // Auto allowed: a waiting card is released by the switch itself, no answer needed
    let mut b = bench(false);
    let live = LiveMode::new(Live::Ask);
    {
        let g = Arc::get_mut(&mut b.gate).unwrap();
        g.live = Some(live.clone());
        g.allow_auto = true;
    }
    let g = b.gate.clone();
    let h = std::thread::spawn(move || g.call_tool("board_add", &json!({"title": "y"}), Some("c2".into())));
    next_approval(&b);
    live.set(Live::Autopilot);
    assert!(h.join().unwrap().is_ok(), "the waiting card was approved by the switch");
    // and the next call does not ask at all
    assert!(b.gate.call_tool("board_add", &json!({"title": "z"}), Some("c3".into())).is_ok());
    // back to Ask: the next risky call asks again
    live.set(Live::Ask);
    let g = b.gate.clone();
    let h = std::thread::spawn(move || g.call_tool("board_add", &json!({"title": "w"}), Some("c4".into())));
    let (id, _, _) = next_approval(&b);
    b.dtx.send((id, Verdict::Deny)).unwrap();
    assert!(h.join().unwrap().is_err());
}
