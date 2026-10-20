//! Where does the assistant's time go? A benchmark of the run loop, before and after the speed and robustness work.
//!
//!   cargo run --release -p iyakki-agent --example assistant_bench -- [sim] [live] [tokens]
//!
//! * `tokens`: the size of what the model is sent for a first request (system prompt + tool descriptions), long vs compact.
//! * `sim`: a scripted model with injected latencies (first token 400 ms, 15 ms per token) and tools that take 350 ms: this
//!   measures OUR overhead (building the request, repairing calls, queues, hand-over) and the effect of parallel calls and
//!   streaming, free of network noise.
//! * `live`: the real Ollama provider configured on this machine (`$IYAKKI_SETTINGS`, default
//!   `~/.local/share/iyakki-dev/settings.json`), with canned tool results so the model and our loop are measured, not the web.
//!   Skipped when the server is not reachable. Nothing is changed in the settings.
//!
//! "before" is `Assist::off()` with the long system prompt and every tool; "after" is `Assist::standard` with the compact prompt,
//! lazy tool groups, the capability reminder, streaming and parallel calls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::agent::{start_with, RunEvent, RunLimits, Tools, UserTools, Verdict, AGENT_SYSTEM_PROMPT};
use iyakki_agent::assistrun::Assist;
use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
use iyakki_agent::providers::{ProviderChat, ProviderClient, ProviderCfg};
use iyakki_agent::services::Services;
use iyakki_agent::toolblock::{self, ReminderCtx, COMPACT_PROMPT};
use iyakki_agent::HostApi;
use iyakki_sec::policy::{BrowserAgent, Role};
use serde_json::{json, Value};

const PROMPTS: [&str; 5] = ["what is the stock price of envx", "weather in chennai", "top stock market news", "play a tamil fm", "summarise this page"];

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "bench".into()
    }
    fn net_status(&self) -> String {
        "up".into()
    }
    fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
        Err("no".into())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// The real tool list of an administrator's account with a screen and a full browser engine.
fn real_tools() -> Arc<UserTools> {
    let dir = std::env::temp_dir().join(format!("iyakki-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
    svc.screen.set_present(true);
    iyakki_agent::browsertools::assume_engine(true);
    let mut p = Role::Admin.template();
    p.browser_agent = BrowserAgent::Interactive;
    p.fs_read = vec![dir.join("home").display().to_string()];
    p.fs_write = vec![dir.join("home").display().to_string()];
    Arc::new(UserTools::new("admin", p, svc, Arc::new(Host), vec![]).with_cwd(&dir.join("home")))
}

fn tokens_of_specs(specs: &[ToolSpec]) -> usize {
    specs.iter().map(|s| (s.name.len() + s.description.len() + s.schema.to_string().len()) / 4).sum()
}

fn print_tokens() {
    let t = real_tools();
    let defs = t.defs();
    let all: Vec<ToolSpec> = defs.iter().map(|d| d.spec.clone()).collect();
    println!("### Prompt size (what a first request carries, estimated at 4 characters a token)\n");
    println!("| | system prompt | tool descriptions | reminder | total | tools offered |");
    println!("|---|---:|---:|---:|---:|---:|");
    println!("| before (long prompt, every tool) | {} | {} | 0 | {} | {} |", AGENT_SYSTEM_PROMPT.len() / 4, tokens_of_specs(&all), AGENT_SYSTEM_PROMPT.len() / 4 + tokens_of_specs(&all), all.len());
    let ctx = ReminderCtx { agent_on: true, browser_policy: "interactive".into(), engine_installed: true, screen_available: true, loaded: None };
    for q in PROMPTS {
        let mut loaded: std::collections::BTreeSet<_> = toolblock::Group::ALL.into_iter().filter(|g| g.always()).collect();
        loaded.extend(toolblock::intent_groups(q));
        let mut specs = toolblock::select_specs(&defs, &loaded);
        specs.push(toolblock::more_tools_spec());
        let rem = toolblock::reminder(&defs, &ReminderCtx { loaded: Some(loaded), ..ctx.clone() });
        let total = COMPACT_PROMPT.len() / 4 + tokens_of_specs(&specs) + rem.len() / 4;
        println!("| after: \"{q}\" | {} | {} | {} | {} | {} |", COMPACT_PROMPT.len() / 4, tokens_of_specs(&specs), rem.len() / 4, total, specs.len());
    }
    println!();
}

// ------------------------------------------------------------------------------------------------ the simulated model

/// A model with a fixed first-token delay and speed. It decides what to do from the user's message.
struct Sim {
    ttft: u64,
    per_token: u64,
    stream: bool,
    calls_made: Mutex<VecDeque<()>>,
}

impl Sim {
    fn script(prompt: &str, round: usize) -> (Vec<ToolCall>, &'static str) {
        let c = |id: &str, name: &str, args: Value| ToolCall { id: id.into(), name: name.into(), args };
        let answer = "Here is what I found, with the date and numbered sources [1] [2]. The figures may be delayed by a few minutes, and one source could not be read because it needs a sign-in. In short: it is steady today, with small moves either way. ";
        if round > 0 {
            return (vec![], answer);
        }
        match prompt {
            p if p.contains("stock price") => (vec![c("1", "stock_quote", json!({"symbol": "ENVX"}))], ""),
            p if p.contains("weather") => (vec![c("1", "weather", json!({"place": "Chennai"}))], ""),
            p if p.contains("news") => (vec![c("1", "web_search", json!({"query": "stock market news"})), c("2", "web_fetch", json!({"url": "https://a.example/1"})), c("3", "web_fetch", json!({"url": "https://b.example/2"})), c("4", "web_fetch", json!({"url": "https://c.example/3"}))], ""),
            p if p.contains("tamil fm") => (vec![c("1", "radio_play", json!({"query": "tamil fm"}))], ""),
            _ => (vec![c("1", "browser_page", json!({}))], ""),
        }
    }
}

impl ChatBackend for Sim {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("unused".into())
    }
    fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.complete_tools_stream(h, t, &mut |_| true)
    }
    fn complete_tools_stream(&self, h: &[Message], _t: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.calls_made.lock().unwrap().push_back(());
        let prompt = h.iter().rev().find(|m| m.role == iyakki_agent::chat::Role::User).map(|m| m.content.as_str()).unwrap_or("");
        let round = h.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).count().min(1);
        let (calls, answer) = Sim::script(prompt, round);
        std::thread::sleep(Duration::from_millis(self.ttft));
        if self.stream {
            for w in answer.split_inclusive(' ') {
                std::thread::sleep(Duration::from_millis(self.per_token));
                on_text(w);
            }
        } else {
            std::thread::sleep(Duration::from_millis(self.per_token * answer.split_whitespace().count() as u64));
        }
        Ok(ModelTurn { text: answer.to_string(), calls })
    }
}

/// Tools that take `latency` and answer with a canned text; the real tool list.
struct Canned {
    inner: Arc<UserTools>,
    latency: Duration,
}

impl Tools for Canned {
    fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
        self.inner.defs()
    }
    fn call(&self, name: &str, _args: &Value) -> Result<String, String> {
        std::thread::sleep(self.latency);
        Ok(match name {
            "stock_quote" => "ENVX (Enovix Corporation) 8.42 USD, +0.31 (+3.8%) at 2026-10-13 15:42 UTC. Source: quote.example. May be delayed.".to_string(),
            "weather" => "Chennai: 31 C, humid, light wind, 40% chance of rain this evening. Source: wx.example.".to_string(),
            "radio_search" => "1. Suryan FM (Tamil) [id suryan-fm] built-in\n2. Radio City Tamil [id radiocity-tamil] built-in".to_string(),
            "radio_play" => "Playing Suryan FM (Tamil) from the built-in list.".to_string(),
            "browser_page" => "The page you have open: Markets today. Text: Shares rose on Tuesday as ...".to_string(),
            _ => "A page of text about markets. ".repeat(60),
        })
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        self.inner.describe(name, args)
    }
}

struct Outcome {
    pre: u64,
    first: u64,
    total: u64,
    model: u64,
    tools: u64,
    ours: u64,
    rounds: u32,
    tool_names: Vec<String>,
    answer: String,
    tokens: u64,
}

fn run_once(backend: Arc<dyn ChatBackend>, tools: Arc<dyn Tools>, history: Vec<Message>, assist: Assist, limit_secs: u64) -> Outcome {
    let t0 = Instant::now();
    let h = start_with(backend, tools, history, RunLimits { max_seconds: limit_secs, ..RunLimits::default() }, assist);
    let mut o = Outcome { pre: 0, first: 0, total: 0, model: 0, tools: 0, ours: 0, rounds: 0, tool_names: vec![], answer: String::new(), tokens: 0 };
    let trace = std::env::var("BENCH_TRACE").is_ok();
    let mut saw_timing = false;
    let mut first_event: Option<u64> = None;
    loop {
        match h.events.recv_timeout(Duration::from_secs(limit_secs + 30)) {
            Ok(e) => {
                if trace && !matches!(e, RunEvent::Delta { .. } | RunEvent::Usage { .. }) {
                    let line = format!("{e:?}");
                    eprintln!("  [{:>6} ms] {}", t0.elapsed().as_millis(), line.chars().take(230).collect::<String>());
                }
                match &e {
                    RunEvent::Delta { .. } if first_event.is_none() => first_event = Some(t0.elapsed().as_millis() as u64),
                    RunEvent::Timing { first_token_ms, first_text_ms, total_ms, model_ms, tools_ms, overhead_ms, pre_request_ms, rounds, .. } => {
                        saw_timing = true;
                        o.pre = *pre_request_ms;
                        o.first = if *first_text_ms > 0 { *first_text_ms } else { *first_token_ms };
                        o.total = *total_ms;
                        o.model = *model_ms;
                        o.tools = *tools_ms;
                        o.ours = *overhead_ms;
                        o.rounds = *rounds;
                    }
                    RunEvent::ToolResult { name, .. } => o.tool_names.push(name.clone()),
                    RunEvent::Usage { tokens, .. } => o.tokens = *tokens,
                    RunEvent::Approval { id, .. } => h.decide(id, Verdict::Approve),
                    RunEvent::Final { text } => {
                        o.answer = text.clone();
                        break;
                    }
                    RunEvent::Failed { error } => {
                        o.answer = format!("FAILED: {error}");
                        break;
                    }
                    RunEvent::Stopped { reason } => {
                        o.answer = format!("STOPPED: {reason}");
                        break;
                    }
                    _ => {}
                }
            }
            Err(_) => {
                o.answer = "TIMED OUT".into();
                break;
            }
        }
    }
    let wall = t0.elapsed().as_millis() as u64;
    if !saw_timing {
        // the safety net is off: the loop does not time itself, so only the wall clock is known
        o.total = wall;
        o.first = first_event.unwrap_or(wall);
    }
    o
}

fn reminder_ctx() -> ReminderCtx {
    ReminderCtx { agent_on: true, browser_policy: "interactive".into(), engine_installed: true, screen_available: true, loaded: None }
}

fn sim() {
    println!("### Simulated model (first token 400 ms, 15 ms per word, tools 350 ms; four of the news tools run together)\n");
    println!("| prompt | before: first text | before: total | after: first text | after: total | after: our overhead | after: before first request |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    let tools = real_tools();
    let canned: Arc<dyn Tools> = Arc::new(Canned { inner: tools, latency: Duration::from_millis(350) });
    for q in PROMPTS {
        let before = run_once(Arc::new(Sim { ttft: 400, per_token: 15, stream: false, calls_made: Default::default() }), canned.clone(), vec![Message::system(AGENT_SYSTEM_PROMPT), Message::user(q)], Assist::off(), 60);
        let after = run_once(Arc::new(Sim { ttft: 400, per_token: 15, stream: true, calls_made: Default::default() }), canned.clone(), vec![Message::system(COMPACT_PROMPT), Message::user(q)], Assist::standard(reminder_ctx()), 60);
        println!("| {q} | {} ms | {} ms | {} ms | {} ms | {} ms | {} ms |", before.first, before.total, after.first, after.total, after.ours, after.pre);
    }
    println!();
}

// ------------------------------------------------------------------------------------------------ the real Ollama

fn live() {
    let path = std::env::var("IYAKKI_SETTINGS").unwrap_or_else(|_| format!("{}/.local/share/iyakki-dev/settings.json", std::env::var("HOME").unwrap_or_default()));
    let Ok(raw) = std::fs::read_to_string(&path) else {
        println!("live: no settings at {path}; skipped\n");
        return;
    };
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let (Some(pid), Some(model)) = (v.pointer("/chat/provider").and_then(Value::as_str), v.pointer("/chat/model").and_then(Value::as_str)) else {
        println!("live: no chat model selected in {path}; skipped\n");
        return;
    };
    // BENCH_MODEL tries another model of the same server (for example one without native tool calling)
    let model_owned = std::env::var("BENCH_MODEL").unwrap_or_else(|_| model.to_string());
    let model = model_owned.as_str();
    let cfg: Option<ProviderCfg> = v.get("providers").and_then(Value::as_array).and_then(|a| a.iter().find(|p| p["id"] == pid)).and_then(|p| serde_json::from_value(p.clone()).ok());
    let Some(cfg) = cfg else {
        println!("live: provider {pid} not found; skipped\n");
        return;
    };
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
    let quick: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(4)));
    let probe = ProviderClient::new(cfg.clone(), iyakki_agent::vault::Secrets::new(None, quick.clone()), quick);
    match probe.list_models() {
        Ok(m) if m.iter().any(|x| x == model) => {}
        Ok(_) => {
            println!("live: {pid} answers but has no model {model}; skipped\n");
            return;
        }
        Err(e) => {
            println!("live: {pid} is not reachable ({e}); skipped\n");
            return;
        }
    }
    let client = || ProviderClient::new(cfg.clone(), iyakki_agent::vault::Secrets::new(None, up.clone()), up.clone());
    let streamer: Arc<dyn iyakki_proxy::Streamer> = Arc::new(iyakki_proxy::https::UreqStreamer::with_response_timeout(Duration::from_secs(600)));
    let warm = client().warm_up(model).unwrap_or(0);
    println!("### Live: {pid} / {model} (warm-up call took {warm} ms; tool results are canned)\n");
    let tools: Arc<dyn Tools> = Arc::new(Canned { inner: real_tools(), latency: Duration::from_millis(0) });
    let plain: Arc<dyn ChatBackend> = Arc::new(ProviderChat { client: client(), model: model.into(), streamer: None });
    let streaming: Arc<dyn ChatBackend> = Arc::new(ProviderChat { client: client(), model: model.into(), streamer: Some(streamer) });
    let only: Option<String> = std::env::var("BENCH_PROMPT").ok();
    println!("| prompt | before: first | before: total | before: tools used | before: tokens | after: first | after: total | after: tools used | after: tokens | after: our overhead | after: before request |");
    println!("|---|---:|---:|---|---:|---:|---:|---|---:|---:|---:|");
    for q in PROMPTS.iter().filter(|q| only.as_ref().is_none_or(|o| q.contains(o.as_str()))) {
        let before = run_once(plain.clone(), tools.clone(), vec![Message::system(AGENT_SYSTEM_PROMPT), Message::user(*q)], Assist::off(), 280);
        let after = run_once(streaming.clone(), tools.clone(), vec![Message::system(COMPACT_PROMPT), Message::user(*q)], Assist::standard(reminder_ctx()), 280);
        println!("| {q} | {} ms | {} ms | {} | {} | {} ms | {} ms | {} | {} | {} ms | {} ms |", before.first, before.total, names(&before), before.tokens, after.first, after.total, names(&after), after.tokens, after.ours, after.pre);
        if before.answer.starts_with("FAILED") || after.answer.starts_with("FAILED") {
            println!("|  | {} | | | | {} | | | | | |", before.answer.chars().take(60).collect::<String>(), after.answer.chars().take(60).collect::<String>());
        }
    }
    println!();
    // the situation of the screenshots: a chat whose history is full of refusals
    println!("### Live: a chat full of earlier refusals, then \"what is the weather in chennai\"\n");
    let poisoned = |sys: &str| {
        let mut h = vec![Message::system(sys)];
        for (q, a) in [("go to gemini website", "I cannot access external websites or services. My capabilities are confined to the tools described earlier (web research, text analysis, planning)."), ("check the stock price of envx", "I cannot check access to external services."), ("take a screenshot", "I cannot take screenshots of my own interface.")] {
            h.push(Message::user(q));
            h.push(Message::assistant(a));
        }
        h.push(Message::user("what is the weather in chennai"));
        h
    };
    let before = run_once(plain.clone(), tools.clone(), poisoned(AGENT_SYSTEM_PROMPT), Assist::off(), 280);
    let defs = tools.defs();
    let names_all: Vec<&str> = defs.iter().map(|d| d.spec.name.as_str()).collect();
    let cond = iyakki_agent::assistant_guard::condense(poisoned(COMPACT_PROMPT), &names_all, 6000, "");
    let after = run_once(streaming.clone(), tools.clone(), cond.history, Assist::standard(reminder_ctx()), 280);
    println!("| | tools used | answer (start) | total |\n|---|---|---|---:|");
    for (n, o) in [("before", &before), ("after", &after)] {
        println!("| {n} | {} | {} | {} ms |", names(o), o.answer.chars().take(90).collect::<String>().replace('\n', " "), o.total);
    }
    println!();
}

fn names(o: &Outcome) -> String {
    if o.tool_names.is_empty() { "none".into() } else { o.tool_names.join(", ") }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.is_empty();
    let want = |w: &str| all || args.iter().any(|a| a == w);
    if want("tokens") {
        print_tokens();
    }
    if want("sim") {
        sim();
    }
    if want("live") {
        live();
    }
}
