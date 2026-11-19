//! Live-model checks (all `#[ignore = "live model"]`): the same run loops the scripted tests drive, but with a real model from a
//! Lemonade server (OpenAI-compatible) on the LAN.
//!
//! Environment (nothing of it is in the repository):
//! * `IYAKKI_LIVE_LEMONADE_URL`       the server's base address;
//! * `IYAKKI_LIVE_LEMONADE_KEY_FILE`  a file holding the API key (read by the provider client at run time, never printed);
//! * `IYAKKI_LIVE_MODEL`              the model (default `qwen3-8b`);
//! * `IYAKKI_LIVE_EVIDENCE_DIR`       where the redacted evidence goes (default `~/iyakki-work/live/2026-11-12/L1`).
//!
//! Run one: `cargo test -p iyakki-agent --locked live_tests::<name> -- --ignored --nocapture`.
//!
//! A model varies, so every check runs up to three attempts and asserts on structure (which tools were called, approvals,
//! files created, board states, stop conditions), never on wording. A check passes when at least two of three attempts hold;
//! the evidence says how many did.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{start_with, Risk, RunEvent, RunHandle, RunLimits, ToolDef, Tools, Verdict};
use crate::assistrun::Assist;
use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::providers::{Kind, ProviderCfg, ProviderChat, ProviderClient};
use crate::toolblock::ReminderCtx;
use crate::vault::Secrets;

// ------------------------------------------------------------------------------------------------ the provider

fn need(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("live model check: set {k} (see the header of live_tests.rs)"))
}

pub(crate) fn model_name() -> String {
    std::env::var("IYAKKI_LIVE_MODEL").unwrap_or_else(|_| "qwen3-8b".into())
}

fn upstream(secs: u64) -> Arc<dyn iyakki_proxy::Upstream> {
    Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(secs)))
}

/// A provider client for the live server. `key_ref` overrides the key (a wrong one, or none); `url` the address.
pub(crate) fn client_with(url: Option<&str>, key_ref: Option<Option<&str>>) -> ProviderClient {
    let mut cfg = ProviderCfg::new("lemonade-lan", Kind::Lemonade);
    cfg.base_url = Some(url.map(str::to_string).unwrap_or_else(|| need("IYAKKI_LIVE_LEMONADE_URL")));
    cfg.key_ref = match key_ref {
        Some(k) => k.map(str::to_string),
        None => Some(format!("file:{}", need("IYAKKI_LIVE_LEMONADE_KEY_FILE"))),
    };
    let up = upstream(240);
    ProviderClient::new(cfg, Secrets::new(None, up.clone()), up)
}

fn chat_for(model: &str) -> ProviderChat {
    ProviderChat { client: client_with(None, None), model: model.to_string(), streamer: None }
}

// ------------------------------------------------------------------------------------------------ evidence

fn evidence_root() -> PathBuf {
    match std::env::var("IYAKKI_LIVE_EVIDENCE_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("iyakki-work/live/2026-11-12/L1"), // home-ok: default evidence folder of the ignored live tests; nothing is read or written without them
    }
}

/// Take the key and the address out of any text that is saved.
fn redact(s: &str) -> String {
    let mut out = s.to_string();
    if let Ok(f) = std::env::var("IYAKKI_LIVE_LEMONADE_KEY_FILE") {
        if let Ok(k) = std::fs::read_to_string(f) {
            let k = k.trim();
            if k.len() >= 6 {
                out = out.replace(k, "<key>");
            }
        }
    }
    if let Ok(u) = std::env::var("IYAKKI_LIVE_LEMONADE_URL") {
        out = out.replace(u.trim_end_matches('/'), "<server>");
        if let Some(h) = iyakki_sec::url_host(&u) {
            out = out.replace(&h, "<host>");
        }
    }
    out
}

pub(crate) fn head(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{t}…") } else { t }
}

/// Wraps the real provider: times every request and keeps a summary of it (sizes, tool names, the tokens the server reported).
pub(crate) struct Rec {
    inner: ProviderChat,
    pub log: Mutex<Vec<Value>>,
}

impl Rec {
    pub(crate) fn new(model: &str) -> Arc<Rec> {
        Arc::new(Rec { inner: chat_for(model), log: Mutex::new(vec![]) })
    }
    pub(crate) fn requests(&self) -> usize {
        self.log.lock().unwrap().len()
    }
    /// Every prompt token count the server reported, in order.
    pub(crate) fn prompt_tokens(&self) -> Vec<u64> {
        self.log.lock().unwrap().iter().filter_map(|v| v["tokens_in"].as_u64()).collect()
    }
    pub(crate) fn totals(&self) -> (u64, u64, u64) {
        let l = self.log.lock().unwrap();
        (l.iter().filter_map(|v| v["tokens_in"].as_u64()).sum(), l.iter().filter_map(|v| v["tokens_out"].as_u64()).sum(), l.iter().filter_map(|v| v["ms"].as_u64()).sum())
    }
}

impl ChatBackend for Rec {
    fn provider_name(&self) -> String {
        "Lemonade (LAN)".into()
    }
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        self.complete_tools(h, &[]).map(|t| t.text)
    }
    fn complete_tools(&self, h: &[Message], t: &[ToolSpec]) -> Result<ModelTurn, String> {
        let start = Instant::now();
        // the LAN can drop for a moment ("No route to host"): ask again a few times before it counts as a failure of the model
        let mut r = self.inner.complete_tools(h, t);
        for _ in 0..4 {
            match &r {
                Err(e) if e.contains("No route to host") || e.contains("Network is unreachable") || e.contains("Connection reset") => {
                    self.log.lock().unwrap().push(json!({"transport_retry": head(e, 80)}));
                    std::thread::sleep(Duration::from_secs(10));
                    r = self.inner.complete_tools(h, t);
                }
                _ => break,
            }
        }
        let ms = start.elapsed().as_millis() as u64;
        let usage = crate::providers::reported_usage();
        let mut e = json!({"ms": ms, "messages": h.len(), "est_prompt_tokens": crate::agent::estimate_tokens(h), "tools_offered": t.len(),
            "tokens_in": usage.map(|u| u.0), "tokens_out": usage.map(|u| u.1)});
        match &r {
            Ok(turn) => {
                e["text_chars"] = json!(turn.text.len());
                e["text_head"] = json!(head(&turn.text, 160));
                e["calls"] = json!(turn.calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
            }
            Err(err) => e["error"] = json!(head(err, 300)),
        }
        self.log.lock().unwrap().push(e);
        r
    }
}

/// What one attempt reports for the evidence file.
#[derive(Default)]
pub(crate) struct Note {
    pub detail: Vec<String>,
}

impl Note {
    pub(crate) fn say(&mut self, s: impl Into<String>) {
        self.detail.push(s.into());
    }
}

/// Run a check up to three times (always three, to measure how reliable it is) and save redacted evidence outside the repository.
/// Panics (fails the test) unless at least two attempts pass.
pub(crate) fn attempts(row: &str, what: &str, f: impl Fn(&Arc<Rec>, &mut Note) -> Result<(), String>) {
    let recs: Vec<Arc<Rec>> = (0..3).map(|_| Rec::new(&model_name())).collect();
    let r = attempts_over(row, what, &recs, |n, note| f(&recs[n], note));
    assert_rows(&[r]);
}

/// Like [`attempts`] for checks that share one run of the model per attempt (the runs are made once and judged by several rows):
/// `recs[n]` is the recorder of attempt `n`.
pub(crate) fn attempts_over(row: &str, what: &str, recs: &[Arc<Rec>], f: impl Fn(usize, &mut Note) -> Result<(), String>) -> (String, usize) {
    // when the server cannot be reached nothing is judged and no evidence is overwritten
    let mut up = false;
    for _ in 0..12 {
        if client_with(None, None).test().is_ok() {
            up = true;
            break;
        }
        std::thread::sleep(Duration::from_secs(10));
    }
    assert!(up, "{row}: the Lemonade server cannot be reached; nothing was run and no evidence was written");
    let dir = evidence_root().join(row);
    let _ = std::fs::create_dir_all(&dir);
    let model = model_name();
    let mut sum = vec![];
    let mut passes = 0;
    for n in 1..=3 {
        let rec = recs[n - 1].clone();
        let mut note = Note::default();
        let t = Instant::now();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(n - 1, &mut note)));
        let res = match res {
            Ok(r) => r,
            Err(p) => Err(format!("panic: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
        };
        let ok = res.is_ok();
        if ok {
            passes += 1;
        }
        let (tin, tout, mms) = rec.totals();
        let report = json!({"row": row, "claim": what, "attempt": n, "model": model, "server": "a Lemonade server on the LAN", "pass": ok,
            "failure": res.as_ref().err().map(|e| head(e, 1200)), "wall_ms": t.elapsed().as_millis() as u64, "model_ms": mms,
            "requests": rec.requests(), "tokens_in": tin, "tokens_out": tout, "notes": note.detail, "request_log": *rec.log.lock().unwrap()});
        let _ = std::fs::write(dir.join(format!("attempt-{n}.json")), redact(&serde_json::to_string_pretty(&report).unwrap()));
        println!("[{row}] attempt {n}: {} in {} ms, {} request(s){}", if ok { "PASS" } else { "FAIL" }, t.elapsed().as_millis(), rec.requests(), res.as_ref().err().map(|e| format!(" — {}", head(e, 300))).unwrap_or_default());
        sum.push(json!({"attempt": n, "pass": ok, "wall_ms": t.elapsed().as_millis() as u64, "tokens_in": tin, "tokens_out": tout, "failure": res.err().map(|e| head(&e, 400))}));
    }
    let s = json!({"row": row, "claim": what, "model": model, "server": "a Lemonade server on the LAN", "passes": passes, "of": 3, "attempts": sum});
    let _ = std::fs::write(dir.join("summary.json"), redact(&serde_json::to_string_pretty(&s).unwrap()));
    println!("[{row}] {passes} of 3 attempts passed");
    (row.to_string(), passes)
}

/// Fail the test when a row had fewer than two passing attempts (after every row of the test has been judged and saved).
pub(crate) fn assert_rows(rows: &[(String, usize)]) {
    let bad: Vec<String> = rows.iter().filter(|r| r.1 < 2).map(|r| format!("{} ({} of 3)", r.0, r.1)).collect();
    assert!(bad.is_empty(), "rows with fewer than two passing attempts: {}", bad.join(", "));
}

// ------------------------------------------------------------------------------------------------ a tool set to drive

pub(crate) type Handler = Box<dyn Fn(&str, &Value) -> Result<String, String> + Send + Sync>;

/// Fake tools with the real names and argument shapes. File tools really write inside `root`.
pub(crate) struct Reg {
    pub defs: Vec<ToolDef>,
    pub ran: Mutex<Vec<(String, Value, bool)>>,
    pub out: Handler,
    /// Calls that policy refuses outright: (tool, the reason given to the model).
    pub refusals: Mutex<Vec<(String, String)>>,
}

pub(crate) fn tdef(name: &str, desc: &str, props: Value, required: &[&str], risk: Risk) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema: json!({"type": "object", "properties": props, "required": required}) }, risk, untrusted: false }
}

impl Reg {
    pub(crate) fn new(defs: Vec<ToolDef>, out: Handler) -> Arc<Reg> {
        Arc::new(Reg { defs, ran: Mutex::new(vec![]), out, refusals: Mutex::new(vec![]) })
    }
    pub(crate) fn names(&self) -> Vec<String> {
        self.ran.lock().unwrap().iter().map(|r| r.0.clone()).collect()
    }
    pub(crate) fn count(&self, name: &str) -> usize {
        self.ran.lock().unwrap().iter().filter(|r| r.0 == name).count()
    }
}

impl Tools for Reg {
    fn defs(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let r = (self.out)(name, args);
        self.ran.lock().unwrap().push((name.to_string(), args.clone(), r.is_ok()));
        r
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        format!("{name} {args}")
    }
    fn refusal(&self, name: &str, _: &Value) -> Option<String> {
        self.refusals.lock().unwrap().iter().find(|r| r.0 == name).map(|r| r.1.clone())
    }
}

fn inside(root: &Path, p: &str) -> Result<PathBuf, String> {
    let rel = Path::new(p);
    if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("refused: every file tool is limited to the workspace folder".into());
    }
    Ok(root.join(rel))
}

/// The file and build tools of a workspace chat. `build` decides what `deploy_build` does with the root folder.
pub(crate) fn workspace_tools(root: &Path, build: Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync>, status: Arc<dyn Fn(&Path) -> Result<String, String> + Send + Sync>) -> Arc<Reg> {
    let defs = vec![
        tdef("read_file", "Read a text file in the workspace.", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
        tdef("list_dir", "List the files of a folder of the workspace (path \".\" is the root).", json!({"path": {"type": "string"}}), &["path"], Risk::Safe),
        tdef("write_file", "Create or replace a text file in the workspace. Missing folders are created.", json!({"path": {"type": "string"}, "content": {"type": "string"}}), &["path", "content"], Risk::Ask),
        tdef("deploy_build", "Build a container image from the Dockerfile in a folder of the workspace.", json!({"dir": {"type": "string"}, "image": {"type": "string"}}), &["dir", "image"], Risk::Ask),
        tdef("deploy_status", "Show whether a deployment is ready.", json!({"name": {"type": "string"}}), &["name"], Risk::Safe),
    ];
    let root = root.to_path_buf();
    Reg::new(
        defs,
        Box::new(move |n, a| {
            let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            match n {
                "read_file" => std::fs::read_to_string(inside(&root, &s("path"))?).map_err(|e| format!("cannot read {}: {e}", s("path"))),
                "list_dir" => {
                    let d = inside(&root, &s("path"))?;
                    let mut v: Vec<String> = std::fs::read_dir(&d).map_err(|e| format!("cannot list {}: {e}", s("path")))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
                    v.sort();
                    Ok(v.join("\n"))
                }
                "write_file" => {
                    let p = crate::scope::resolve_for_write(&root, &s("path"))?;
                    crate::scope::ensure_parent(&p)?;
                    std::fs::write(&p, s("content")).map_err(|e| crate::scope::explain_io(&e, &p, "write"))?;
                    Ok(format!("wrote {} ({} bytes)", s("path"), s("content").len()))
                }
                "deploy_build" => build(&root),
                "deploy_status" => status(&root),
                o => Err(format!("unknown tool {o}")),
            }
        }),
    )
}

fn ctx() -> ReminderCtx {
    ReminderCtx { agent_on: true, browser_policy: "interactive".into(), engine_installed: true, screen_available: false, loaded: None }
}

pub(crate) fn drive(h: &RunHandle, secs: u64, mut verdict: impl FnMut(&str) -> Option<Verdict>) -> Vec<RunEvent> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match h.events.recv_timeout(Duration::from_millis(100)) {
            Ok(e) => {
                if let RunEvent::Approval { id, name, .. } = &e {
                    if let Some(v) = verdict(name) {
                        h.decide(id, v);
                    }
                }
                let done = matches!(e, RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. });
                out.push(e);
                if done {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    if !matches!(out.last(), Some(RunEvent::Final { .. } | RunEvent::Failed { .. } | RunEvent::Stopped { .. })) {
        h.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    out
}

pub(crate) fn final_of(e: &[RunEvent]) -> Result<String, String> {
    match e.last() {
        Some(RunEvent::Final { text }) => Ok(text.clone()),
        Some(RunEvent::Failed { error }) => Err(format!("the run failed: {}", head(error, 300))),
        Some(RunEvent::Stopped { reason }) => Err(format!("the run was stopped: {reason}")),
        _ => Err("the run did not end in time".into()),
    }
}

pub(crate) fn notes_of(e: &[RunEvent]) -> Vec<String> {
    e.iter().filter_map(|x| if let RunEvent::Note { text } = x { Some(text.clone()) } else { None }).collect()
}

pub(crate) fn tmp_root(tag: &str, n: usize) -> PathBuf {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-live-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// One plain run of the assistant loop with a real model.
pub(crate) fn run_chat<T: Tools + 'static>(rec: &Arc<Rec>, tools: Arc<T>, system: &str, ask: &str, auto: bool, secs: u64, verdict: impl FnMut(&str) -> Option<Verdict>) -> Vec<RunEvent> {
    let limits = RunLimits { auto, max_steps: 14, max_seconds: secs, ..RunLimits::default() };
    let h = start_with(rec.clone(), tools, vec![Message::system(system), Message::user(ask)], limits, Assist::standard(ctx()));
    drive(&h, secs + 20, verdict)
}

// ------------------------------------------------------------------------------------------------ the checks

mod assistant_rows;
mod autopilot_rows;
mod workspace_rows;
mod provider_rows;
mod cloud_rows;
