//! The assistant's tools for FLOWS. Reads are Safe. Making, editing, running and cancelling ask. Publishing, rolling back, changing
//! other runs and widening the agent's own autonomy are hard-ask: they ask in every mode, Auto and Autopilot included (see
//! `agent::hard_ask`). Nothing here approves a fix the agent proposed: that is the person's button. Credentials never pass through
//! these tools: configs hold `{{secret.NAME}}` references, and everything shown to the model is redacted.

use std::sync::Arc;

use iyakki_flow::store::PublishReq;
use iyakki_flow::*;
use iyakki_sec::policy::{App, Policy};
use serde_json::{json, Value};

use super::*;
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::services::Services;

pub const MAX_CHARS: usize = 6000;

pub fn is_flow_tool(name: &str) -> bool {
    matches!(name, "flow_list" | "flow_get" | "flow_runs" | "flow_run_status" | "flow_logs" | "flow_explain_failure" | "flow_create" | "flow_edit" | "flow_run" | "flow_cancel" | "flow_publish" | "flow_rollback" | "flow_apply_to_runs" | "flow_set_heal_policy")
}

pub fn offered(p: &Policy) -> bool {
    p.apps.contains(&App::Flows)
}

fn tool(name: &str, desc: &str, schema: Value, risk: Risk, untrusted: bool) -> ToolDef {
    ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted }
}

fn obj(props: Value, req: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": req})
}

const NODE_HELP: &str = "A node: {id, kind, name?, config, retry?, timeout_ms?, runner?, heal?, on_error?}. Kinds and their config: http {method,url,headers,body,expect_status,extract}; cli {command:[program,args...] (never a shell), cwd, env, parse (text|json|lines|regex), retry_exit_codes, retry_patterns, cli:{id,min_version}}; grpc {address,method,data,plaintext,metadata,deadline_ms} (uses grpcurl); browser {steps,allow_domains,level (read_only|interactive),extract}; agent {prompt,output_schema,max_cents} (no tools yet); script {lang:js|wasm,source|package,args}; condition {expr} (ports true/false); switch {value,cases}; delay {seconds}; approval {message,timeout_s}; notify {title,body,level}; subflow {flow,inputs}; foreach {items,flow,max_items}; remote {url,auth:{kind,secret}}. BROWSER QA kinds (one private browser session per run, closed when the run ends; selectors are CSS; secrets only as `vault:NAME` in text fields; put `defaults.browser` {trusted,profile:isolated|person,max_actions,max_minutes} on the flow): br_open {url,wait:load|idle|none}; br_screenshot {name,selector?,index?,full_page?} (output.path is a PNG file); br_dom {mode:page|outer_html|text|attributes|summary,selector?,max_bytes,console,links,failures}; br_assert {check: exists|not_exists|count|text_equals|text_contains|text_matches|attr_equals|attr_contains|value_equals|visible|hidden|enabled|disabled|checked|url_equals|url_contains|title_equals|title_contains|no_console_errors|page_contains|page_not_contains, selector?, expected?, attribute?, soft?, wait_ms} (output.passed, output.message); br_click {selector,native?}; br_type {selector,text,clear,press_enter}; br_fill {fields:{selector:value},submit?}; br_submit {selector?}; br_key {key,selector?}; br_hover; br_scroll {to:selector|top|bottom|down|up}; br_select {selector,option,by}; br_wait {for:selector|selector_gone|text|text_gone|url|time,value,ms}; br_extract {fields:{name:selector|{selector,attr,all}},rows:{selector,columns}}; br_js {expression,args} (administrators only); br_frame {action:frame|parent|main|tabs,selector}; br_viewport {preset|width,height}; br_storage {kind:cookie|local_storage|session_storage,name,value}; br_download {name_contains,save_in}; br_compare {name,threshold,tolerance,update} (baseline picture, saved on the first run). An agent step can show screenshots to a vision-capable model with `images: [\"{{ nodes.shot.output.path }}\"]`. Use {{ nodes.<id>.output.<field> }} and {{ inputs.<name> }} in config; credentials only as {{secret.NAME}} (a vault secret the person stores, never a value). thrift is planned and cannot run. retry: {attempts, backoff:{kind:fixed,delay_ms} | {kind:exponential,base_ms,factor,max_ms,jitter} | {kind:list,steps:[{timeout_ms,delay_ms}]}}.";

pub fn defs(p: &Policy) -> Vec<ToolDef> {
    if !offered(p) {
        return vec![];
    }
    let flow_arg = json!({"type": "string", "description": "the flow's id (from flow_list); omit for the flow open on screen"});
    vec![
        tool("flow_list", "List the person's workflows (FLOWS): id, name, published version, whether the draft differs, whether the agent changed it, node count. Start here.", obj(json!({}), &[]), Risk::Safe, false),
        tool("flow_get", "Describe one flow: its nodes with kind and settings, edges, triggers, retry and heal policy, and what is wrong with it (validation). `version`: draft (default), published, or a number. Credentials are never shown.", obj(json!({"flow": flow_arg.clone(), "version": {}, "config": {"type": "boolean", "description": "include each node's settings (default true)"}}), &[]), Risk::Safe, true),
        tool("flow_runs", "List runs, newest first: id, flow, version, status (scheduled, queued, running, retrying, healing, blocked = needs you, failed, succeeded, cancelled), progress and the error or what it waits for. Filter by `flow` and `status`.", obj(json!({"flow": {"type": "string"}, "status": {"type": "string"}, "limit": {"type": "integer"}}), &[]), Risk::Safe, true),
        tool("flow_run_status", "One run in detail: each step's state, attempts (n of max, the limit and the pause of each), the error, what it waits for, the fix cards and who decided them.", obj(json!({"run": {"type": "string"}}), &["run"]), Risk::Safe, true),
        tool("flow_logs", "The log of a run (redacted), optionally one step's, the last `tail` lines (default 60, at most 300).", obj(json!({"run": {"type": "string"}, "node": {"type": "string"}, "tail": {"type": "integer"}}), &["run"]), Risk::Safe, true),
        tool("flow_explain_failure", "Why a run failed or is stuck, from its data: the failing step, every attempt with its limit, pause and error, what the numbers suggest, and any fix the agent proposed. Quote it; do not guess beyond it.", obj(json!({"run": {"type": "string"}}), &["run"]), Risk::Safe, true),
        tool("flow_create", &format!("Make a new DRAFT flow (nothing runs until it is published). Give `name`, optional `description`, `nodes` (a start node is added if missing; with no `edges` they are chained in order), `edges` [{{from,to,port?,when?}}], `triggers` [{{kind: manual|cron|interval|once|calendar|file_change|on_failure|event, expr?, tz?, every_s?, ...}}], `inputs` [{{name,type,required,default}}], `defaults`. Or `template` (health-check, nightly-backup, build-test-deploy, site-monitor, grpc-call, daily-report, qa-login-smoke, qa-responsive-shots, qa-page-health) with `name`. {NODE_HELP} Asks the person first."), obj(json!({"name": {"type": "string"}, "description": {"type": "string"}, "template": {"type": "string"}, "nodes": {"type": "array", "items": {"type": "object"}}, "edges": {"type": "array", "items": {"type": "object"}}, "chain": {"type": "boolean"}, "triggers": {"type": "array", "items": {"type": "object"}}, "inputs": {"type": "array", "items": {"type": "object"}}, "defaults": {"type": "object"}}), &[]), Risk::Ask, false),
        tool("flow_edit", &format!("Change a DRAFT flow (never a published version): `ops` is a list of {{op: add_node|update_node|remove_node|add_edge|remove_edge|set_trigger|remove_trigger|set_default|set_meta|set_input, ...}}. update_node merges `config` keys (null removes one) and sets retry, timeout_ms, runner, heal, track, on_error. add_node may carry `after` (connect from that node, with `port`) and `before`. The change is marked as the agent's until the person publishes. {NODE_HELP} Asks the person first."), obj(json!({"flow": flow_arg.clone(), "ops": {"type": "array", "items": {"type": "object"}}}), &["ops"]), Risk::Ask, false),
        tool("flow_run", "Start a run now, or later with `at` (\"in 10m\", \"2026-10-20 15:30\" local time). Runs the latest published version; `draft: true` runs the draft to try it. `inputs` is an object; `key` is an idempotency key (the same key never starts a second run). Asks the person first.", obj(json!({"flow": flow_arg.clone(), "inputs": {"type": "object"}, "at": {"type": "string"}, "draft": {"type": "boolean"}, "key": {"type": "string"}}), &[]), Risk::Ask, false),
        tool("flow_cancel", "Cancel a run (a running command ends at its own timeout). Asks the person first.", obj(json!({"run": {"type": "string"}}), &["run"]), Risk::Ask, false),
        tool("flow_publish", "Publish the draft as the next immutable version, with an optional `label` and a `changelog` line. The person always approves, also in Auto mode, and approves any change the agent made.", obj(json!({"flow": flow_arg.clone(), "label": {"type": "string"}, "changelog": {"type": "string"}}), &["changelog"]), Risk::Ask, false),
        tool("flow_rollback", "Publish an older version's content as a new version. The person always approves.", obj(json!({"flow": flow_arg.clone(), "version": {"type": "integer"}}), &["version"]), Risk::Ask, false),
        tool("flow_apply_to_runs", "Give scheduled and running runs the latest published definition of one node: `scope` scheduled (now), running (at the node's next boundary, never mid-node) or both. Incompatible changes (other ports or output names) are refused unless `allow_incompatible`. The person always approves.", obj(json!({"flow": flow_arg.clone(), "node": {"type": "string"}, "scope": {"type": "string", "enum": ["scheduled", "running", "both"]}, "allow_incompatible": {"type": "boolean"}}), &["node", "scope"]), Risk::Ask, false),
        tool("flow_set_heal_policy", "Set who may fix failed steps: `mode` off, suggest (the agent proposes, the person approves), auto_fix_simple (simple fixes are applied by themselves), always_ask. Optional `node` for one node, `max_per_run`, `max_per_day`, `cost_cap_cents`, `auto_publish_simple`. Anything but `off` always asks the person.", obj(json!({"flow": flow_arg, "node": {"type": "string"}, "mode": {"type": "string", "enum": ["off", "suggest", "auto_fix_simple", "always_ask"]}, "max_per_run": {"type": "integer"}, "max_per_day": {"type": "integer"}, "cost_cap_cents": {"type": "integer"}, "auto_publish_simple": {"type": "boolean"}}), &["mode"]), Risk::Ask, false),
    ]
}

pub fn describe(name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let flow = if s("flow").is_empty() { "the open flow".to_string() } else { format!("flow {}", s("flow")) };
    match name {
        "flow_create" => format!("make a draft flow “{}”{}", s("name"), if s("template").is_empty() { String::new() } else { format!(" from the {} template", s("template")) }),
        "flow_edit" => format!("change the draft of {flow} ({} operation(s))", args.get("ops").and_then(Value::as_array).map_or(0, Vec::len)),
        "flow_run" => format!("run {flow}{}{}", if s("at").is_empty() { " now".to_string() } else { format!(" at {}", s("at")) }, if args.get("draft").and_then(Value::as_bool).unwrap_or(false) { " (the draft)" } else { "" }),
        "flow_cancel" => format!("cancel run {}", s("run")),
        "flow_publish" => format!("PUBLISH the draft of {flow} as the next version: {}", s("changelog")),
        "flow_rollback" => format!("ROLL BACK {flow} to version {}, published as a new version", args.get("version").and_then(Value::as_i64).unwrap_or(0)),
        "flow_apply_to_runs" => format!("give {} runs of {flow} the newest definition of step {}", s("scope"), s("node")),
        "flow_set_heal_policy" => format!("let the agent {} for {}{}", match s("mode").as_str() { "off" => "not fix anything".to_string(), "suggest" => "propose fixes you approve".to_string(), "auto_fix_simple" => "apply simple fixes by itself".to_string(), "always_ask" => "ask you before every fix".to_string(), o => o.to_string() }, flow, if s("node").is_empty() { String::new() } else { format!(", step {}", s("node")) }),
        other => format!("{other} {}", s("flow")),
    }
}

fn cut(s: String) -> String {
    if s.len() <= MAX_CHARS {
        return s;
    }
    let mut end = MAX_CHARS;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (cut)", &s[..end])
}

fn s<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("")
}

/// "in 10m", "+2h", "2026-10-20 15:30" (local time) to epoch milliseconds.
pub fn parse_when(text: &str, now_ms: i64) -> Result<i64, String> {
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() || t == "now" {
        return Ok(now_ms);
    }
    let rel = t.strip_prefix("in ").or_else(|| t.strip_prefix('+')).unwrap_or("").trim();
    if !rel.is_empty() {
        let digits: String = rel.chars().take_while(|c| c.is_ascii_digit()).collect();
        let unit = rel[digits.len()..].trim();
        let n: i64 = digits.parse().map_err(|_| format!("`{text}` is not a time (try \"in 10m\" or \"2026-10-20 15:30\")"))?;
        let mult = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86400,
            _ => return Err(format!("`{text}`: the unit is s, m, h or d")),
        };
        return Ok(now_ms + n * mult * 1000);
    }
    let (t, _) = crate::calendar::parse_local(&crate::clock::Zone::system(), text.trim()).map_err(|e| format!("`{text}` is not a time ({e})"))?;
    Ok(t * 1000)
}

impl FlowEnv {
    fn flow_id(&self, args: &Value, open: Option<&SharedOpen>) -> Result<String, String> {
        let f = s(args, "flow");
        if !f.is_empty() {
            return Ok(f.to_string());
        }
        open.and_then(|o| o.lock().unwrap_or_else(|e| e.into_inner()).flow.clone()).ok_or_else(|| "give `flow` (the id from flow_list)".into())
    }

    fn draft(&self, id: &str) -> Result<Flow, String> {
        self.store.get_draft(id).ok_or_else(|| format!("there is no flow `{id}` (flow_list shows them)"))
    }

    /// Run a tool call. `open` is the flow the tile has open (the default `flow`).
    pub fn call(&self, name: &str, args: &Value, open: Option<&SharedOpen>) -> Result<String, String> {
        if !allowed(&self.policy) {
            return Err("your account may not use FLOWS".into());
        }
        let now = self.engine_now();
        match name {
            "flow_list" => {
                let v = self.summaries();
                if v.is_empty() {
                    return Ok("There are no flows yet. flow_create makes one (templates: health-check, nightly-backup, build-test-deploy, site-monitor, grpc-call, daily-report).".into());
                }
                Ok(cut(v.iter().map(|f| format!("{} \"{}\" published v{}{}{} {} nodes, {} triggers{}", f.id, f.name, f.published, if f.draft_changed { ", draft changed" } else { "" }, if f.agent_changed { ", agent changed" } else { "" }, f.nodes, f.triggers, if f.production { ", production" } else { "" })).collect::<Vec<_>>().join("\n")))
            }
            "flow_get" => {
                let id = self.flow_id(args, open)?;
                let which = args.get("version");
                let f = match which {
                    Some(Value::Number(n)) => self.store.get_version(&id, n.as_u64().unwrap_or(0) as u32).ok_or_else(|| format!("flow `{id}` has no version {n}"))?,
                    Some(Value::String(t)) if t == "published" => self.store.published(&id).ok_or_else(|| format!("flow `{id}` is not published yet"))?,
                    _ => self.draft(&id)?,
                };
                let mut out = flow_brief(&f, args.get("config").and_then(Value::as_bool).unwrap_or(true));
                let issues = self.issue_lines(&f);
                if issues.is_empty() {
                    out.push_str("Validation: no problems.\n");
                } else {
                    out.push_str("Validation:\n");
                    for i in issues.iter().take(15) {
                        out.push_str(&format!("  {i}\n"));
                    }
                }
                let ups = self.engine.node_updates(&id);
                for u in ups {
                    out.push_str(&format!("{} scheduled and {} running runs use an older definition of step {} ({}).\n", u.scheduled.len(), u.running.len(), u.node, if u.compatible { "compatible" } else { "NOT compatible: other ports or outputs" }));
                }
                Ok(cut(out))
            }
            "flow_runs" => {
                let id = s(args, "flow");
                let st = s(args, "status").to_ascii_lowercase();
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(15).clamp(1, 50) as usize;
                let v: Vec<String> = self.engine.summaries().iter().filter(|r| id.is_empty() || r.flow_id == id).filter(|r| st.is_empty() || r.status.label().replace(' ', "_") == st || r.status.label() == st || (st == "blocked" && r.status == RunStatus::Blocked)).take(limit).map(run_line).collect();
                Ok(if v.is_empty() { "No runs match.".into() } else { cut(v.join("\n")) })
            }
            "flow_run_status" => {
                let r = self.engine.get_run(s(args, "run")).ok_or_else(|| format!("there is no run `{}`", s(args, "run")))?;
                let mut out = format!("{}\n", run_line(&r.summary()));
                out.push_str(&format!("trigger {} by {}; started {}; heal fixes used {}\n", r.trigger.kind, r.trigger.by, r.started_ms, r.heal_spent));
                for n in &r.flow.nodes {
                    if let Some(nr) = r.nodes.get(&n.id) {
                        out.push_str(&format!("  {} [{}] {} attempts {}/{}{}{}\n", n.id, n.kind, nr.state.label(), nr.attempts.len(), nr.max_attempts, if nr.error.is_empty() { String::new() } else { format!(" error: {}", nr.error.chars().take(160).collect::<String>()) }, if nr.note.is_empty() { String::new() } else { format!(" ({})", nr.note) }));
                    }
                }
                if r.flow.nodes.iter().any(|n| n.kind.starts_with("br_")) {
                    let rep = iyakki_flow::report::build(&r);
                    out.push_str(&format!("QA report: {} ({} checks, {} failed, {} soft; {} screenshots, {} console errors)\n", if rep.passed { "PASSED" } else { "NOT PASSED" }, rep.summary.checks, rep.summary.checks_failed, rep.summary.soft_failed, rep.summary.screenshots, rep.summary.console_errors));
                    for n in rep.nodes.iter().filter(|n| n.kind.starts_with("br_")) {
                        if let Some(a) = &n.assertion {
                            out.push_str(&format!("  check {}: {} {}\n", n.id, if a["passed"].as_bool() == Some(true) { "passed" } else { "FAILED" }, a["message"].as_str().unwrap_or("")));
                        }
                        for e in n.console_errors.iter().take(3) {
                            out.push_str(&format!("  console {}: {}\n", n.id, e.chars().take(160).collect::<String>()));
                        }
                    }
                    for g in rep.gallery.iter().take(10) {
                        out.push_str(&format!("  picture {}: {}\n", g.node, g.path));
                    }
                }
                for c in &r.heal {
                    out.push_str(&format!("  fix card {} for {}: {:?}{}{}\n", c.id, c.node, c.state, c.patch.as_ref().map(|p| format!(" {}", p.summary())).unwrap_or_default(), if c.decided_by.is_empty() { String::new() } else { format!(" decided by {}", c.decided_by) }));
                }
                Ok(cut(out))
            }
            "flow_logs" => {
                let id = s(args, "run");
                let r = self.engine.get_run(id).ok_or_else(|| format!("there is no run `{id}`"))?;
                let tail = args.get("tail").and_then(Value::as_u64).unwrap_or(60).clamp(1, 300) as usize;
                let node = s(args, "node");
                let lines: Vec<String> = if node.is_empty() { r.log.iter().map(|l| format!("{}{}", if l.node.is_empty() { String::new() } else { format!("[{}] ", l.node) }, l.text)).collect() } else { r.nodes.get(node).map(|n| n.log.clone()).ok_or_else(|| format!("no step `{node}`"))? };
                let from = lines.len().saturating_sub(tail);
                Ok(cut(lines[from..].join("\n")))
            }
            "flow_explain_failure" => {
                let r = self.engine.get_run(s(args, "run")).ok_or_else(|| format!("there is no run `{}`", s(args, "run")))?;
                Ok(explain_failure(&r))
            }
            "flow_create" => {
                let by = AGENT;
                let f = if !s(args, "template").is_empty() { self.from_template(s(args, "template"), s(args, "name"), by)? } else { self.create_draft(build_flow(args)?, by)? };
                self.host_audit("flow.create", &format!("{} by {by}", f.id));
                if let Some(o) = open {
                    o.lock().unwrap_or_else(|e| e.into_inner()).flow = Some(f.id.clone());
                }
                let issues = self.issue_lines(&f);
                Ok(format!("Made the draft `{}` ({} nodes). It is a draft: nothing runs until it is published (and the person approves that).{}", f.id, f.nodes.len(), if issues.is_empty() { String::new() } else { format!("\nProblems to fix:\n{}", issues.join("\n")) }))
            }
            "flow_edit" => {
                let id = self.flow_id(args, open)?;
                let before = self.draft(&id)?;
                let mut f = before.clone();
                let ops = args.get("ops").and_then(Value::as_array).ok_or("give `ops`")?;
                if ops.len() > 40 {
                    return Err("at most 40 operations at a time".into());
                }
                let done = apply_ops(&mut f, ops)?;
                let touched = ops.iter().filter_map(|o| o.get("id").and_then(Value::as_str)).next().unwrap_or("");
                self.save_edit(&f, AGENT, &done.join("; "), touched)?;
                self.host_audit("flow.edit", &format!("{id} by {AGENT}: {}", done.join("; ")));
                let issues = self.issue_lines(&f);
                Ok(format!("Changed the draft of `{id}` (marked as the agent's): {}.{}", done.join("; "), if issues.is_empty() { " No problems found.".into() } else { format!("\nProblems:\n{}", issues.join("\n")) }))
            }
            "flow_run" => {
                let id = self.flow_id(args, open)?;
                let at = if s(args, "at").is_empty() { None } else { Some(parse_when(s(args, "at"), now)?) };
                let mut req = StartRequest::new(&id).trigger("chat", AGENT);
                req.inputs = args.get("inputs").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
                req.at_ms = at;
                req.draft = args.get("draft").and_then(Value::as_bool).unwrap_or(false);
                if !s(args, "key").is_empty() {
                    req.idempotency_key = Some(s(args, "key").to_string());
                }
                let o = self.engine.start_run(req)?;
                Ok(if o.existing { format!("A run with that key already exists: {}.", o.run_id) } else if at.is_some_and(|a| a > now) { format!("Scheduled run {}.", o.run_id) } else { format!("Started run {}. flow_run_status shows how it goes.", o.run_id) })
            }
            "flow_cancel" => {
                self.engine.cancel(s(args, "run"))?;
                Ok(format!("Cancelling run {}.", s(args, "run")))
            }
            "flow_publish" => {
                let id = self.flow_id(args, open)?;
                // the person approved this call, so the agent's changes in the draft are approved with it
                let v = self.store.publish(&id, &PublishReq { by: &self.who(), label: s(args, "label"), changelog: s(args, "changelog"), approved_by: Some(&self.who()) })?;
                self.host_audit("flow.publish", &format!("{id} v{v} by {} (approved)", self.who()));
                let ups = self.engine.node_updates(&id);
                Ok(format!("Published `{id}` as version {v}.{}", if ups.is_empty() { String::new() } else { format!(" {} step(s) have runs on an older definition; flow_apply_to_runs can move them over.", ups.len()) }))
            }
            "flow_rollback" => {
                let id = self.flow_id(args, open)?;
                let to = args.get("version").and_then(Value::as_u64).ok_or("give `version`")? as u32;
                let v = self.store.rollback(&id, to, &self.who())?;
                self.host_audit("flow.rollback", &format!("{id} to v{to} as v{v} by {}", self.who()));
                Ok(format!("Rolled `{id}` back to version {to}; published as version {v}."))
            }
            "flow_apply_to_runs" => {
                let id = self.flow_id(args, open)?;
                let scope = match s(args, "scope") {
                    "scheduled" => ApplyScope::Scheduled,
                    "running" => ApplyScope::Running,
                    "both" => ApplyScope::Both,
                    o => return Err(format!("`{o}`: scope is scheduled, running or both")),
                };
                let r = self.engine.apply_node_update(&id, s(args, "node"), scope, &self.who(), args.get("allow_incompatible").and_then(Value::as_bool).unwrap_or(false))?;
                Ok(format!("{} scheduled run(s) updated now; {} running run(s) will take it at the step's next boundary; {} already past the step; {} not changed because the change is incompatible.", r.scheduled_updated.len(), r.running_pending.len(), r.skipped_started.len(), r.skipped_incompatible.len()))
            }
            "flow_set_heal_policy" => {
                let id = self.flow_id(args, open)?;
                let mode = HealMode::parse(s(args, "mode")).ok_or("mode is off, suggest, auto_fix_simple or always_ask")?;
                let mut f = self.draft(&id)?;
                let node = s(args, "node").to_string();
                let cur = if node.is_empty() { f.defaults.heal.clone() } else { f.node(&node).ok_or_else(|| format!("no step `{node}`"))?.heal.clone().unwrap_or_else(|| f.defaults.heal.clone()) };
                let mut p = cur.clone();
                p.mode = mode;
                if let Some(n) = args.get("max_per_run").and_then(Value::as_u64) {
                    p.max_per_run = n.min(20) as u32;
                }
                if let Some(n) = args.get("max_per_day").and_then(Value::as_u64) {
                    p.max_per_day = n.min(100) as u32;
                }
                if let Some(n) = args.get("cost_cap_cents").and_then(Value::as_u64) {
                    p.cost_cap_cents = n.min(10_000);
                }
                if let Some(b) = args.get("auto_publish_simple").and_then(Value::as_bool) {
                    p.auto_publish_simple = b;
                }
                if node.is_empty() {
                    f.defaults.heal = p;
                } else if let Some(n) = f.node_mut(&node) {
                    n.heal = Some(p);
                }
                self.save_edit(&f, AGENT, &format!("heal policy {}", mode.label()), &node)?;
                self.host_audit("flow.heal_policy", &format!("{id}{} -> {} by {AGENT} (approved)", if node.is_empty() { String::new() } else { format!("/{node}") }, mode.label()));
                Ok(format!("The heal policy of `{id}`{} is now: {}. (A draft change: publish it for scheduled runs to use it.)", if node.is_empty() { String::new() } else { format!(" step {node}") }, mode.label()))
            }
            other => Err(format!("`{other}` is not a flows tool")),
        }
    }

    pub fn preview(&self, name: &str, args: &Value, open: Option<&SharedOpen>) -> Option<String> {
        match name {
            "flow_edit" => {
                let id = self.flow_id(args, open).ok()?;
                let before = self.draft(&id).ok()?;
                let mut f = before.clone();
                apply_ops(&mut f, args.get("ops")?.as_array()?).ok()?;
                Some(store::diff(&before, &f).text())
            }
            "flow_publish" => {
                let id = self.flow_id(args, open).ok()?;
                let d = self.draft(&id).ok()?;
                let text = match self.store.published(&id) {
                    Some(p) => store::diff(&p, &d).text(),
                    None => format!("first version: {} nodes, {} edges", d.nodes.len(), d.edges.len()),
                };
                let agent: Vec<String> = d.meta.modified_by.iter().filter(|c| c.by_agent()).map(|c| format!("- {}", c.summary)).collect();
                Some(format!("{text}{}", if agent.is_empty() { String::new() } else { format!("\nChanges made by the agent in this draft:\n{}", agent.join("\n")) }))
            }
            "flow_rollback" => {
                let id = self.flow_id(args, open).ok()?;
                let to = args.get("version")?.as_u64()? as u32;
                let cur = self.store.published(&id)?;
                let old = self.store.get_version(&id, to)?;
                Some(store::diff(&cur, &old).text())
            }
            _ => None,
        }
    }

    fn host_audit(&self, action: &str, detail: &str) {
        iyakki_flow::host::Host::audit(self.host.as_ref(), action, detail);
    }

    fn engine_now(&self) -> i64 {
        crate::clock::now_unix() * 1000
    }
}

pub fn standalone_call(name: &str, args: &Value, svc: &Arc<Services>, p: &Policy, user: &str) -> Result<String, String> {
    FlowEnv::open(svc, user, p)?.call(name, args, None)
}

pub fn standalone_preview(name: &str, args: &Value, svc: &Arc<Services>, p: &Policy, user: &str) -> Option<String> {
    FlowEnv::open(svc, user, p).ok()?.preview(name, args, None)
}

// ------------------------------------------------------------------------------------------------ the open flow's chat

/// Wraps the chat's ordinary tools with the flows tools bound to what the tile has open.
pub struct FlowTools {
    inner: Arc<dyn Tools>,
    env: Arc<FlowEnv>,
    open: SharedOpen,
}

impl FlowTools {
    pub fn new(inner: Arc<dyn Tools>, env: Arc<FlowEnv>, open: SharedOpen) -> FlowTools {
        FlowTools { inner, env, open }
    }

    /// The picture the harness puts in the system prompt for every message.
    pub fn context(env: &FlowEnv, open: &SharedOpen) -> String {
        context_text(env, &open.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The rules for the model inside the FLOWS tile (kept short).
pub const PROMPT_RULES: &str = "\nYou work in FLOWS, the person's workflow tile (what is open is described below, refreshed with every message). To build or change a flow use flow_create and flow_edit: they make DRAFTS, shown on the canvas at once, marked as yours, and the person publishes them. Never publish by yourself: ask with flow_publish only when the person says to, and it will ask them. Look before you change: flow_get first, then edit step by step (ids, ports and `{{ nodes.<id>.output.<field> }}` references must match). For \"why did run N fail\" call flow_explain_failure and quote its numbers; do not invent causes. Retry settings go in the node's `retry` (attempts, and backoff fixed, exponential or a per-attempt list of timeout and pause). Commands are a list of arguments, never a shell string. Credentials: never ask for, repeat or write one; use {{secret.NAME}} and, when the secret is missing, call vault_request so the person types it into their vault. You cannot approve your own fix: when a run needs the person, say so and point them to the card. Board and feature-row rules do not apply here.";

impl Tools for FlowTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_flow_tool(&d.spec.name));
        v.extend(defs(&self.env.policy));
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if is_flow_tool(name) { self.env.call(name, args, Some(&self.open)) } else { self.inner.call(name, args) }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        if is_flow_tool(name) { describe(name, args) } else { self.inner.describe(name, args) }
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if is_flow_tool(name) { self.env.preview(name, args, Some(&self.open)) } else { self.inner.preview(name, args) }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if is_flow_tool(name) { None } else { self.inner.refusal(name, args) }
    }
    fn pre_approved(&self, name: &str) -> bool {
        !is_flow_tool(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        // publishing and the other hard-ask calls are decided each time
        if is_flow_tool(name) { !matches!(name, "flow_publish" | "flow_rollback" | "flow_apply_to_runs" | "flow_set_heal_policy") } else { self.inner.approved_always(name) }
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
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
}
