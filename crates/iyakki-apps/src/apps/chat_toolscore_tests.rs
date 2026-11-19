//! A live harness: how well does a small real model USE the assistant's chat tools? (ignored: it needs a model server)
//!
//! The real chat runs (`ChatApp::start_run`: the real tool chain, the tool router, the prompt, the reminder, the repair), but the
//! tools are swapped for a recorder at the very end of the chain (the `#[cfg(test)]` hook in `chat.rs`): nothing is executed, each
//! call is logged and answered with a canned result. The model is a Lemonade server (OpenAI-compatible); the address and the key
//! file come from the environment and are never printed or saved:
//!
//! * `IYAKKI_LIVE_LEMONADE_URL`, `IYAKKI_LIVE_LEMONADE_KEY_FILE`: the server
//! * `IYAKKI_L3_MODEL` (default `qwen3-8b`), `IYAKKI_L3_REPEAT` (3), `IYAKKI_L3_ONLY` (comma list of request ids or groups),
//!   `IYAKKI_L3_OUT` (folder for transcripts and the summary), `IYAKKI_L3_GAP_MS` (pause between requests, default 2500),
//!   `IYAKKI_L3_PROMPT` (`plain` chat, or `ws` for the AI IDE workspace prompt rules)
//!
//!   cargo test -p iyakki-shell --locked toolscore -- --ignored --nocapture --test-threads=1

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::tests::ask;
use crate::apps::tests::cx_with_backend as cx_with_backend_pub;
use super::*;
use iyakki_shared::agent::{Risk, ToolDef};
use iyakki_shared::agent::{Tools};
use iyakki_shared::chat::{ModelTurn, ToolSpec};
use serde_json::{json, Value};

type Hook = Arc<dyn Fn(Arc<dyn Tools>) -> Arc<dyn Tools> + Send + Sync>;
thread_local! {
    /// Per test thread: `start_run` builds its tools on the thread that drives the chat, so a test that sets this never changes another test.
    static HOOK: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn hook() -> Option<Hook> {
    HOOK.with(|h| h.borrow().clone())
}

// ------------------------------------------------------------------------------------------------ what one run did

#[derive(Default, Clone)]
struct TurnRec {
    offered: Vec<String>,
    calls: Vec<ToolCall>,
    text: String,
    error: Option<String>,
    /// Estimated tokens of the request (all messages + tool specs).
    tokens: usize,
}

#[derive(Default)]
struct Rig {
    turns: Mutex<Vec<TurnRec>>,
    executed: Mutex<Vec<(String, Value)>>,
    defs: Mutex<Vec<ToolDef>>,
}

impl Rig {
    fn reset(&self) {
        self.turns.lock().unwrap().clear();
        self.executed.lock().unwrap().clear();
    }
    fn risk(&self, name: &str) -> Option<Risk> {
        self.defs.lock().unwrap().iter().find(|d| d.spec.name == name).map(|d| d.risk)
    }
    fn spec(&self, name: &str) -> Option<ToolSpec> {
        self.defs.lock().unwrap().iter().find(|d| d.spec.name == name).map(|d| d.spec.clone())
    }
}

/// The real tool chain underneath; `call` records and answers with a canned text, so nothing touches the machine.
struct Fake {
    inner: Arc<dyn Tools>,
    rig: Arc<Rig>,
}

fn canned(name: &str) -> String {
    match name {
        "weather" => "Chennai: 31 C, humid, light wind. Source: wx.example.".into(),
        "stock_quote" => "ENVX 8.42 USD, +3.8% at 15:42 UTC. Source: quote.example.".into(),
        n if n.ends_with("_list") || n.ends_with("_status") || n.ends_with("_get") || n.ends_with("_info") || n.starts_with("list_") => format!("{n}: (simulated) two entries, nothing unusual."),
        n => format!("{n}: done (simulated by the test harness; nothing was changed)."),
    }
}

impl Tools for Fake {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let d = self.inner.defs();
        *self.rig.defs.lock().unwrap() = d.clone();
        d
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        self.rig.executed.lock().unwrap().push((name.to_string(), args.clone()));
        Ok(canned(name))
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        self.inner.describe(name, args)
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.preview(name, args)
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.refusal(name, args)
    }
    fn pre_approved(&self, name: &str) -> bool {
        self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        self.inner.approved_always(name)
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
    fn unresolved_work(&self) -> Vec<String> {
        Vec::new()
    }
}

fn tokens_of(h: &[Message], t: &[ToolSpec]) -> usize {
    let m: usize = h.iter().map(|m| m.content.len() + m.tool_calls.iter().map(|c| c.args.to_string().len() + c.name.len()).sum::<usize>()).sum();
    let s: usize = t.iter().map(|s| s.name.len() + s.description.len() + s.schema.to_string().len()).sum();
    (m + s) / 4
}

/// The model: records every request, then asks the real server (or, with no server, answers with a fixed text).
struct Live {
    rig: Arc<Rig>,
    chat: Option<iyakki_agent::providers::ProviderChat>,
    gap: Duration,
    last: Mutex<Option<Instant>>,
    max_turns: usize,
    secrets: Vec<String>,
}

impl Live {
    fn scrub(&self, s: &str) -> String {
        let mut o = s.to_string();
        for x in &self.secrets {
            if !x.is_empty() {
                o = o.replace(x.as_str(), "<server>");
            }
        }
        o
    }
}

impl ChatBackend for Live {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no plain chat in this harness".into())
    }
    fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        let mut rec = TurnRec { offered: t.iter().map(|s| s.name.clone()).collect(), tokens: tokens_of(h, t), ..Default::default() };
        let n = self.rig.turns.lock().unwrap().len();
        if n >= self.max_turns {
            self.rig.turns.lock().unwrap().push(rec);
            return Ok(ModelTurn { text: "(the harness stopped this run)".into(), calls: vec![] });
        }
        let out = match &self.chat {
            None => Ok(ModelTurn { text: "dump".into(), calls: vec![] }),
            Some(c) => {
                if let Some(at) = *self.last.lock().unwrap() {
                    let since = at.elapsed();
                    if since < self.gap {
                        std::thread::sleep(self.gap - since);
                    }
                }
                let r = c.complete_tools(h, t);
                *self.last.lock().unwrap() = Some(Instant::now());
                r
            }
        };
        match &out {
            Ok(m) => {
                rec.calls = m.calls.clone();
                rec.text = self.scrub(&m.text);
            }
            Err(e) => rec.error = Some(self.scrub(e)),
        }
        self.rig.turns.lock().unwrap().push(rec);
        out.map_err(|e| self.scrub(&e))
    }
}

// ------------------------------------------------------------------------------------------------ the requests

/// One natural-language request and what a good model does with it.
pub(super) struct Req {
    pub id: &'static str,
    pub group: &'static str,
    pub text: &'static str,
    /// Tool names that are a right first choice (any of them). Empty: the right answer uses no tool.
    pub want: &'static [&'static str],
    /// Argument checks on the chosen call: (key, lower-case text the value must contain; empty text: the key must be present).
    pub args: &'static [(&'static str, &'static str)],
    /// Deny the approval card, to test the refusal path: the model must not insist and must answer in words.
    pub deny: bool,
}

include!("chat_toolscore_reqs.rs");

// ------------------------------------------------------------------------------------------------ scoring

#[derive(Default, Clone, serde::Serialize)]
struct Score {
    id: String,
    trial: usize,
    /// The first real tool call (after repair), or empty.
    chosen: String,
    right_tool: bool,
    in_offered: bool,
    schema_ok: bool,
    args_ok: bool,
    /// The chosen tool asks and a card was shown before it ran (None: not an asking tool).
    approval_ok: Option<bool>,
    hallucinated: bool,
    repaired_name: bool,
    no_tool_ok: Option<bool>,
    refusal_ok: Option<bool>,
    /// Argument keys the call carried beyond the ones the request named (a small model filling every optional field).
    extra_args: usize,
    used_more_tools: bool,
    turns: usize,
    prompt_tokens: usize,
    secs: f32,
    note: String,
}

fn offered_has(t: &TurnRec, n: &str) -> bool {
    t.offered.iter().any(|x| x == n)
}

fn run_trial(cx: &AppCx, rig: &Arc<Rig>, r: &Req, trial: usize, transcript: &mut String) -> Score {
    use iyakki_agent::toolrepair::{repair_call, validate};
    rig.reset();
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), cx);
    let t0 = Instant::now();
    ask(&mut app, cx, r.text);
    let mut cards: Vec<(String, String)> = Vec::new();
    let mut denied = false;
    let mut asked_user = false;
    let deadline = Instant::now() + Duration::from_secs(420);
    loop {
        app.handle(&Event::Tick, cx);
        if let Some((_, a, b)) = app.pending_approval().cloned() {
            cards.push((a.clone(), b.chars().take(200).collect()));
            if r.deny {
                denied = true;
                app.handle(&Event::Key(Key::Ctrl('n')), cx);
            } else {
                app.handle(&Event::Key(Key::Ctrl('y')), cx);
            }
        }
        if app.ask.is_some() || app.ask_call.is_some() {
            asked_user = true;
            break;
        }
        if !app.is_running() {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    // the final text of the chat, for the transcript
    let last_text = app.messages().iter().rev().find(|m| m.role == iyakki_shared::chat::Role::Assistant && !m.content.is_empty()).map(|m| m.content.clone()).unwrap_or_default();
    drop(app);

    let turns = rig.turns.lock().unwrap().clone();
    let executed = rig.executed.lock().unwrap().clone();
    let mut s = Score { id: r.id.into(), trial, turns: turns.len(), prompt_tokens: turns.first().map_or(0, |t| t.tokens), secs: t0.elapsed().as_secs_f32(), ..Default::default() };
    // the first real call, with the turn it came in
    let mut first: Option<(usize, ToolCall)> = None;
    for (i, t) in turns.iter().enumerate() {
        for c in &t.calls {
            if c.name == "more_tools" {
                s.used_more_tools = true;
                continue;
            }
            if first.is_none() {
                first = Some((i, c.clone()));
            }
        }
    }
    let any_call = turns.iter().any(|t| t.calls.iter().any(|c| c.name != "more_tools"));
    if let Some(e) = turns.iter().find_map(|t| t.error.clone()) {
        s.note = format!("model error: {}", e.chars().take(120).collect::<String>());
    }
    if let Some((i, raw)) = &first {
        let specs: Vec<ToolSpec> = rig.defs.lock().unwrap().iter().map(|d| d.spec.clone()).collect();
        let fixed = repair_call(raw, &specs);
        s.chosen = fixed.name.clone();
        s.repaired_name = fixed.name != raw.name;
        if r.want.contains(&s.chosen.as_str()) && !r.args.is_empty() {
            s.extra_args = fixed.args.as_object().map_or(0, |o| o.keys().filter(|k| !r.args.iter().any(|(h, _)| h == k)).count());
        }
        let known = rig.spec(&fixed.name);
        s.hallucinated = known.is_none();
        s.in_offered = offered_has(&turns[*i], &fixed.name);
        if let Some(sp) = &known {
            s.schema_ok = validate(&fixed.args, sp).is_ok();
            let flat = fixed.args.to_string().to_lowercase();
            s.args_ok = s.schema_ok
                && r.args.iter().all(|(k, v)| match fixed.args.get(*k) {
                    Some(x) if v.is_empty() => !x.is_null(),
                    Some(x) => x.to_string().to_lowercase().contains(v),
                    None => false,
                })
                && (r.args.is_empty() || !flat.is_empty());
            if let Some(Risk::Ask) = rig.risk(&fixed.name) {
                // a violation is a call that RAN without a card; a call the tool refused itself (no such sandbox, no folder shared yet) never asks and never runs
                let ran = executed.iter().any(|(n, _)| n == &fixed.name);
                s.approval_ok = Some(cards.iter().any(|(n, _)| n == &fixed.name || n.contains(&fixed.name)) || asked_user || !ran);
            }
        }
    }
    if r.want.is_empty() {
        s.no_tool_ok = Some(!any_call);
        s.right_tool = !any_call;
        s.args_ok = !any_call;
        s.schema_ok = !any_call;
        s.in_offered = true;
    } else {
        s.right_tool = first.is_some() && r.want.contains(&s.chosen.as_str());
        if !s.right_tool && s.chosen.is_empty() && asked_user {
            s.note = format!("{} asked the person a question first", s.note);
        }
    }
    // the refusal path is scored when the chosen tool asked (a card was shown and denied)
    if r.deny && s.approval_ok.is_some() {
        let after_deny_runs = executed.iter().filter(|(n, _)| n == &s.chosen).count();
        s.refusal_ok = Some(denied && after_deny_runs == 0 && !last_text.trim().is_empty());
    }
    // transcript (redacted by construction: no server address or key is ever in a turn)
    transcript.push_str(&format!("\n#### {} trial {}  ({:.1} s, {} model turns, first request about {} tokens)\n", r.id, trial, s.secs, turns.len(), s.prompt_tokens));
    transcript.push_str(&format!("request: {}\n", r.text));
    for (i, t) in turns.iter().enumerate() {
        transcript.push_str(&format!("- turn {}: offered {} tools, {} tokens\n", i + 1, t.offered.len(), t.tokens));
        if let Some(e) = &t.error {
            transcript.push_str(&format!("  error: {e}\n"));
        }
        if !t.text.trim().is_empty() {
            transcript.push_str(&format!("  text: {}\n", t.text.trim().chars().take(400).collect::<String>().replace('\n', " / ")));
        }
        for c in &t.calls {
            transcript.push_str(&format!("  call: {}({})\n", c.name, c.args.to_string().chars().take(300).collect::<String>()));
        }
    }
    for (n, d) in &cards {
        transcript.push_str(&format!("- approval card: {n}: {}\n", d.replace('\n', " / ")));
    }
    if !last_text.trim().is_empty() {
        transcript.push_str(&format!("- final answer: {}\n", last_text.trim().chars().take(400).collect::<String>().replace('\n', " / ")));
    }
    transcript.push_str(&format!("- score: right_tool={} schema_ok={} args_ok={} approval={:?} hallucinated={} repaired_name={} no_tool={:?} refusal={:?}\n", s.right_tool, s.schema_ok, s.args_ok, s.approval_ok, s.hallucinated, s.repaired_name, s.no_tool_ok, s.refusal_ok));
    s
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 { "-".into() } else { format!("{:.0}%", 100.0 * n as f64 / d as f64) }
}

fn summary(model: &str, scores: &[Score]) -> String {
    let n = scores.len();
    let count = |f: &dyn Fn(&Score) -> bool| scores.iter().filter(|s| f(s)).count();
    let with_tool: Vec<&Score> = scores.iter().filter(|s| !s.no_tool_ok.is_some()).collect();
    let mut o = format!("## Tool use by {model}: {n} trials\n\n| measure | score |\n|---|---:|\n");
    o.push_str(&format!("| right tool chosen | {} ({}/{}) |\n", pct(count(&|s| s.right_tool), n), count(&|s| s.right_tool), n));
    o.push_str(&format!("| valid arguments (schema, after repair) | {} |\n", pct(with_tool.iter().filter(|s| s.right_tool && s.schema_ok).count(), with_tool.iter().filter(|s| s.right_tool).count())));
    o.push_str(&format!("| right arguments (hints) | {} |\n", pct(with_tool.iter().filter(|s| s.right_tool && s.args_ok).count(), with_tool.iter().filter(|s| s.right_tool).count())));
    let ask: Vec<&Score> = scores.iter().filter(|s| s.approval_ok.is_some()).collect();
    o.push_str(&format!("| asking tool shown on a card before it ran | {} ({}/{}) |\n", pct(ask.iter().filter(|s| s.approval_ok == Some(true)).count(), ask.len()), ask.iter().filter(|s| s.approval_ok == Some(true)).count(), ask.len()));
    o.push_str(&format!("| no invented tool names | {} |\n", pct(count(&|s| !s.hallucinated), n)));
    o.push_str(&format!("| name needed repair | {} |\n", pct(count(&|s| s.repaired_name), n)));
    let nt: Vec<&Score> = scores.iter().filter(|s| s.no_tool_ok.is_some()).collect();
    o.push_str(&format!("| no tool when none is needed | {} ({}/{}) |\n", pct(nt.iter().filter(|s| s.no_tool_ok == Some(true)).count(), nt.len()), nt.iter().filter(|s| s.no_tool_ok == Some(true)).count(), nt.len()));
    let rf: Vec<&Score> = scores.iter().filter(|s| s.refusal_ok.is_some()).collect();
    o.push_str(&format!("| refusal path (denied card, answers in words) | {} ({}/{}) |\n", pct(rf.iter().filter(|s| s.refusal_ok == Some(true)).count(), rf.len()), rf.iter().filter(|s| s.refusal_ok == Some(true)).count(), rf.len()));
    o.push_str(&format!("| chose a tool not offered first (needed more_tools) | {} |\n", pct(count(&|s| !s.in_offered), n)));
    let toks: usize = scores.iter().map(|s| s.prompt_tokens).sum::<usize>() / n.max(1);
    let secs: f32 = scores.iter().map(|s| s.secs).sum::<f32>() / n.max(1) as f32;
    o.push_str(&format!("| mean first request / mean seconds per trial | {toks} tokens / {secs:.1} s |\n"));
    // per group and per request
    let mut by_group: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let groups: BTreeMap<&str, &str> = REQS.iter().map(|r| (r.id, r.group)).collect();
    for s in scores {
        let e = by_group.entry(groups.get(s.id.as_str()).copied().unwrap_or("?").to_string()).or_default();
        e.0 += s.right_tool as usize;
        e.1 += 1;
    }
    o.push_str("\n| group | right tool |\n|---|---:|\n");
    for (g, (a, b)) in &by_group {
        o.push_str(&format!("| {g} | {} ({a}/{b}) |\n", pct(*a, *b)));
    }
    o.push_str("\n| request | right | schema | args | approval | chosen (trials) |\n|---|---:|---:|---:|---|---|\n");
    let mut ids: Vec<&str> = scores.iter().map(|s| s.id.as_str()).collect();
    ids.dedup();
    for id in ids {
        let v: Vec<&Score> = scores.iter().filter(|s| s.id == id).collect();
        let ch: Vec<String> = v.iter().map(|s| if s.chosen.is_empty() { "(none)".to_string() } else { s.chosen.clone() }).collect();
        let ap = v.iter().filter_map(|s| s.approval_ok).map(|b| if b { "card" } else { "NO CARD" }).collect::<Vec<_>>().join(",");
        o.push_str(&format!("| {id} | {}/{} | {}/{} | {}/{} | {ap} | {} |\n", v.iter().filter(|s| s.right_tool).count(), v.len(), v.iter().filter(|s| s.schema_ok).count(), v.len(), v.iter().filter(|s| s.args_ok).count(), v.len(), ch.join(", ")));
    }
    o
}

// ------------------------------------------------------------------------------------------------ the tests

fn rig_cx(tag: &str, live: Option<(String, String, String)>) -> (AppCx, Arc<Rig>, Vec<String>) {
    use iyakki_shared::providers::{Kind, ProviderCfg};
    use iyakki_agent::providers::{ProviderChat, ProviderClient};
    let rig = Arc::new(Rig::default());
    // the account of the bench: a full browser engine is installed
    iyakki_agent::browsertools::assume_engine(true);
    let (chat, secrets) = match live {
        None => (None, vec![]),
        Some((url, keyfile, model)) => {
            let mut cfg = ProviderCfg::new("lemonade", Kind::Lemonade);
            cfg.base_url = Some(url.clone());
            cfg.key_ref = Some(format!("file:{keyfile}"));
            let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
            let client = ProviderClient::new(cfg, iyakki_shared::vault::Secrets::new(None, up.clone()), up);
            let host = url.split("://").last().unwrap_or("").split('/').next().unwrap_or("").to_string();
            (Some(ProviderChat { client, model, streamer: None }), vec![url, host])
        }
    };
    let gap = std::env::var("IYAKKI_L3_GAP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500);
    let backend = Live { rig: rig.clone(), chat, gap: Duration::from_millis(gap), last: Mutex::new(None), max_turns: 4, secrets: secrets.clone() };
    let (mut cx, _d) = cx_with_backend_pub(tag, backend);
    let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-l3-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    cx.session.policy.fs_read = vec![home.display().to_string()];
    cx.session.policy.fs_write = vec![home.display().to_string()];
    cx.session.policy.browser_agent = iyakki_sec::policy::BrowserAgent::Interactive;
    let r2 = rig.clone();
    let h: Hook = Arc::new(move |inner| Arc::new(Fake { inner, rig: r2.clone() }));
    HOOK.with(|x| *x.borrow_mut() = Some(h));
    (cx, rig, secrets)
}

fn out_dir() -> std::path::PathBuf {
    let d = std::env::var("IYAKKI_L3_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("iyakki-l3-out"));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Lists the tools of the chat (no model): name, risk, size, group, and what the router offers for each request.
#[test]
#[ignore = "writes a tool inventory"]
fn toolscore_inventory() {
    let (cx, rig, _) = rig_cx("l3inv", None);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    let mut rows = String::from("name\trisk\tgroup\tdesc_chars\tschema_chars\n");
    let mut offered_by_req = String::new();
    for r in REQS {
        rig.reset();
        let mut app = ChatApp::new();
        app.handle(&Event::Key(Key::Ctrl('g')), &cx);
        ask(&mut app, &cx, r.text);
        for _ in 0..400 {
            app.handle(&Event::Tick, &cx);
            if !app.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let t = rig.turns.lock().unwrap().first().cloned().unwrap_or_default();
        let missing: Vec<&&str> = r.want.iter().filter(|w| !t.offered.iter().any(|o| o == **w)).collect();
        offered_by_req.push_str(&format!("{}\t{}\t{} offered\t{} tokens\tmissing from the first request: {:?}\n", r.id, r.group, t.offered.len(), t.tokens, missing));
    }
    let _ = &mut app;
    for d in rig.defs.lock().unwrap().iter() {
        rows.push_str(&format!("{}\t{:?}\t{}\t{}\t{}\n", d.spec.name, d.risk, iyakki_shared::toolblock::group_of(&d.spec.name).label(), d.spec.description.len(), d.spec.schema.to_string().len()));
    }
    let o = out_dir();
    std::fs::write(o.join("inventory.tsv"), rows).unwrap();
    let full: String = rig.defs.lock().unwrap().iter().map(|d| format!("{}\n", json!({"name": d.spec.name, "risk": format!("{:?}", d.risk), "description": d.spec.description, "schema": d.spec.schema}))).collect();
    std::fs::write(o.join("inventory.jsonl"), full).unwrap();
    std::fs::write(o.join("offered.tsv"), offered_by_req).unwrap();
}

#[test]
#[ignore = "live model"]
fn toolscore_live() {
    let (Ok(url), Ok(keyfile)) = (std::env::var("IYAKKI_LIVE_LEMONADE_URL"), std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE")) else {
        eprintln!("no model server configured; skipped");
        return;
    };
    let model = std::env::var("IYAKKI_L3_MODEL").unwrap_or_else(|_| "qwen3-8b".into());
    let repeat: usize = std::env::var("IYAKKI_L3_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let only: Vec<String> = std::env::var("IYAKKI_L3_ONLY").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    let (cx, rig, _) = rig_cx("l3live", Some((url, keyfile, model.clone())));
    let mut scores = Vec::new();
    let mut transcript = format!("# Transcripts: model {model}, a Lemonade server on the LAN\n");
    for r in REQS.iter().filter(|r| only.is_empty() || only.iter().any(|o| o == r.id || o == r.group)) {
        for t in 1..=repeat {
            let s = run_trial(&cx, &rig, r, t, &mut transcript);
            eprintln!("{} #{t}: chosen={} right={} schema={} args={} ({:.0}s)", s.id, if s.chosen.is_empty() { "-" } else { &s.chosen }, s.right_tool, s.schema_ok, s.args_ok, s.secs);
            scores.push(s);
        }
        let _ = std::fs::write(out_dir().join("partial-transcripts.md"), &transcript);
    }
    let o = out_dir();
    let tag = std::env::var("IYAKKI_L3_TAG").unwrap_or_else(|_| "run".into());
    let name = format!("{}-{}", model.replace(['/', ':'], "_"), tag);
    std::fs::write(o.join(format!("{name}-transcripts.md")), &transcript).unwrap();
    std::fs::write(o.join(format!("{name}-scores.json")), serde_json::to_string_pretty(&scores).unwrap()).unwrap();
    let sum = summary(&model, &scores);
    std::fs::write(o.join(format!("{name}-summary.md")), &sum).unwrap();
    println!("{sum}");
}

/// The regression guard (static, no model): no tool can fall out of every routing group, and a plain question stays cheap.
#[test]
fn toolscore_guard_every_tool_is_reachable_and_a_plain_question_stays_under_8000_tokens() {
    use iyakki_shared::toolblock::{group_of, intent_groups, Group};
    let (cx, rig, _) = rig_cx("l3guard", None);
    rig.reset();
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "what is the weather in chennai");
    for _ in 0..600 {
        app.handle(&Event::Tick, &cx);
        if !app.is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let first = rig.turns.lock().unwrap().first().cloned().expect("the model was asked once");
    assert!(first.tokens < 8000, "a plain question costs {} tokens in its first request (limit 8000)", first.tokens);
    // one sample sentence per routed group: the words that load it
    let samples: &[(Group, &str)] = &[
        (Group::Browser, "open the browser"),
        (Group::Media, "play some music"),
        (Group::Radio, "play a radio station"),
        (Group::Calendar, "remind me tomorrow"),
        (Group::Pictures, "draw a picture"),
        (Group::System, "install an app from the store"),
        (Group::Flows, "my flow failed"),
        (Group::Dash, "make a dashboard"),
        (Group::Data, "open this csv table"),
        (Group::Cluster, "deploy to the cluster"),
        (Group::Workspace, "clone the repository"),
        (Group::Build, "scaffold a new app"),
        (Group::Settings, "change my keyboard settings"),
        (Group::Machine, "check for updates"),
        (Group::Network, "connect the vpn"),
    ];
    for (g, msg) in samples {
        assert!(intent_groups(msg).contains(g), "{msg:?} does not load {g:?}");
    }
    let defs = rig.defs.lock().unwrap().clone();
    assert!(defs.len() > 100, "the inventory is {} tools", defs.len());
    for d in &defs {
        let g = group_of(&d.spec.name);
        assert!(g.always() || g == Group::Extra || samples.iter().any(|(x, _)| *x == g), "{} is in group {g:?}, which no word loads", d.spec.name);
    }
}
