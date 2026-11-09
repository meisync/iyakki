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
    matches!(name, "flow_list" | "flow_get" | "flow_runs" | "flow_run_status" | "flow_logs" | "flow_explain_failure" | "flow_create" | "flow_edit" | "flow_run" | "flow_cancel" | "flow_publish" | "flow_rollback" | "flow_apply_to_runs" | "flow_set_heal_policy" | "flow_delete" | "flow_drop_version" | "flow_prune_history") || is_webhook_tool(name)
}

pub fn is_webhook_tool(name: &str) -> bool {
    matches!(name, "webhook_list" | "webhook_create" | "webhook_regenerate" | "webhook_enable" | "webhook_disable" | "webhook_public")
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

const NODE_HELP: &str = "A node: {id, kind, name?, config, retry?, timeout_ms?, runner?, heal?, on_error?}. Kinds and their config: http {method,url,headers,body,expect_status,extract}; cli {command:[program,args...] (never a shell), cwd, env, parse (text|json|lines|regex), retry_exit_codes, retry_patterns, cli:{id,min_version}}; grpc {address,method,data,plaintext,metadata,deadline_ms} (uses grpcurl); browser {steps,allow_domains,level (read_only|interactive),extract}; agent {prompt,output_schema,max_cents} (no tools yet); script {lang:js|wasm,source|package,args}; condition {expr} (ports true/false); switch {value,cases}; delay {seconds}; approval {message,timeout_s}; notify {title,body,level}; subflow {flow,inputs}; foreach {items,flow,max_items}; remote {url,auth:{kind,secret}}. BROWSER QA kinds (one private browser session per run, closed when the run ends; selectors are CSS; secrets only as `vault:NAME` in text fields; put `defaults.browser` {trusted,profile:isolated|person,max_actions,max_minutes} on the flow): br_open {url,wait:load|idle|none}; br_screenshot {name,selector?,index?,full_page?} (output.path is a PNG file); br_dom {mode:page|outer_html|text|attributes|summary,selector?,max_bytes,console,links,failures}; br_assert {check: exists|not_exists|count|text_equals|text_contains|text_matches|attr_equals|attr_contains|value_equals|visible|hidden|enabled|disabled|checked|url_equals|url_contains|title_equals|title_contains|no_console_errors|page_contains|page_not_contains, selector?, expected?, attribute?, soft?, wait_ms} (output.passed, output.message); br_click {selector,native?}; br_type {selector,text,clear,press_enter}; br_fill {fields:{selector:value},submit?}; br_submit {selector?}; br_key {key,selector?}; br_hover; br_scroll {to:selector|top|bottom|down|up}; br_select {selector,option,by}; br_wait {for:selector|selector_gone|text|text_gone|url|time,value,ms}; br_extract {fields:{name:selector|{selector,attr,all}},rows:{selector,columns}}; br_js {expression,args} (administrators only); br_frame {action:frame|parent|main|tabs,selector}; br_viewport {preset|width,height}; br_storage {kind:cookie|local_storage|session_storage,name,value}; br_download {name_contains,save_in}; br_compare {name,threshold,tolerance,update} (baseline picture, saved on the first run). CAMERA kind: cam_photo {device?,keep?} takes one picture (output.path is a JPEG, output.taken_at) and runs only when the person switched on `defaults.camera` (Camera allowed; you cannot); the picture is deleted when the run ends unless keep is on; show it to a vision model with an agent step `images`. An agent step can show screenshots to a vision-capable model with `images: [\"{{ nodes.shot.output.path }}\"]`. Use {{ nodes.<id>.output.<field> }} and {{ inputs.<name> }} in config; credentials only as {{secret.NAME}} (a vault secret the person stores, never a value). thrift is planned and cannot run. retry: {attempts, backoff:{kind:fixed,delay_ms} | {kind:exponential,base_ms,factor,max_ms,jitter} | {kind:list,steps:[{timeout_ms,delay_ms}]}}.";

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
        tool("flow_delete", "Delete a whole flow: its draft and every published version go; its runs stay in the history. Refused while the flow has runs that are running or scheduled. The person always confirms, with the flow named.", obj(json!({"flow": flow_arg.clone()}), &[]), Risk::Ask, false),
        tool("flow_drop_version", "Drop one published version of a flow (its content is lost; runs keep their own copy). The current published version is never dropped: publish or roll back to another first. Refused while an active run uses that version. The person always confirms, with the flow and version named.", obj(json!({"flow": flow_arg.clone(), "version": {"type": "integer"}}), &["version"]), Risk::Ask, false),
        tool("flow_prune_history", "Remove old FINISHED runs from the history (their logs and results go; the flows stay). `keep` = how many finished runs of each flow stay (default: the tenant's history_keep, else 200), `days` = finished runs older than this go (default: the tenant's history_days, else 30; 0 = no age limit). The newest finished run of each flow, runs still going or waiting for someone, and runs with an open fix card are never removed. `dry_run: true` only lists what would go. The person always confirms, and sees the list first.", obj(json!({"keep": {"type": "integer"}, "days": {"type": "integer"}, "dry_run": {"type": "boolean"}}), &[]), Risk::Ask, false),
        tool("flow_set_heal_policy", "Set who may fix failed steps: `mode` off, suggest (the agent proposes, the person approves), auto_fix_simple (simple fixes are applied by themselves), always_ask. Optional `node` for one node, `max_per_run`, `max_per_day`, `cost_cap_cents`, `auto_publish_simple`. Anything but `off` always asks the person.", obj(json!({"flow": flow_arg, "node": {"type": "string"}, "mode": {"type": "string", "enum": ["off", "suggest", "auto_fix_simple", "always_ask"]}, "max_per_run": {"type": "integer"}, "max_per_day": {"type": "integer"}, "cost_cap_cents": {"type": "integer"}, "auto_publish_simple": {"type": "boolean"}}), &["mode"]), Risk::Ask, false),
        tool("webhook_list", "List the person's webhooks (an authenticated local HTTP address that starts a flow): id, flow, on or off, signed or not, whether the flow is published, the last delivery. Never shows an address or a secret.", obj(json!({"flow": {"type": "string", "description": "only this flow's"}}), &[]), Risk::Safe, false),
        tool("webhook_create", "Add a webhook to a flow's draft and make its secret address (and, when `signed` (default yes), a shared secret for HMAC-SHA256 signatures). The address and secret are shown ONLY on the person's screen: you never get them, so never ask for them or guess them; tell the person to open the flow's Webhook panel (key w). The hook is OFF and fires nothing until the flow is published and webhook_enable is called. Optional: `methods` (POST default; GET, PUT, PATCH), `max_body` bytes (default 16384, at most 65536), `content_types` (json default, form, text), `rate_per_min` (default 30). The request body reaches the flow as untrusted data under the input `webhook`. Asks the person first.", obj(json!({"flow": flow_arg.clone(), "signed": {"type": "boolean"}, "methods": {"type": "array", "items": {"type": "string"}}, "max_body": {"type": "integer"}, "content_types": {"type": "array", "items": {"type": "string"}}, "rate_per_min": {"type": "integer"}}), &[]), Risk::Ask, false),
        tool("webhook_regenerate", "Make a webhook's secret address (and shared secret) again; the old ones stop working at once. The new ones are shown only on the person's screen. Asks the person first.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Ask, false),
        tool("webhook_enable", "Switch a webhook on. Only works when the flow is published with the trigger and the person may run flows. Asks the person first.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Ask, false),
        tool("webhook_disable", "Switch a webhook off: it answers 404 from then on. Asks the person first.", obj(json!({"id": {"type": "string"}}), &["id"]), Risk::Ask, false),
        tool("webhook_public", "Let a webhook take senders from OUTSIDE the local network for `minutes` (at most 10080 = 7 days; 0 closes it again). Always asks, in every approval mode.", obj(json!({"id": {"type": "string"}, "minutes": {"type": "integer"}}), &["id", "minutes"]), Risk::Ask, false),
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
        "flow_delete" => format!("DELETE {flow} with all its versions (this cannot be undone)"),
        "flow_drop_version" => format!("DROP version {} of {flow} (its content is lost)", args.get("version").and_then(Value::as_i64).unwrap_or(0)),
        "flow_prune_history" => format!("{} finished runs from the history{}", if args.get("dry_run").and_then(Value::as_bool).unwrap_or(false) { "list" } else { "REMOVE" }, match (args.get("keep").and_then(Value::as_u64), args.get("days").and_then(Value::as_u64)) { (None, None) => " (by the tenant's history rule)".to_string(), (k, d) => format!(" (keep {} per flow, older than {} days)", k.map_or("the rule's".to_string(), |k| k.to_string()), d.map_or("the rule's".to_string(), |d| d.to_string())) }),
        "flow_apply_to_runs" => format!("give {} runs of {flow} the newest definition of step {}", s("scope"), s("node")),
        "flow_set_heal_policy" => format!("let the agent {} for {}{}", match s("mode").as_str() { "off" => "not fix anything".to_string(), "suggest" => "propose fixes you approve".to_string(), "auto_fix_simple" => "apply simple fixes by itself".to_string(), "always_ask" => "ask you before every fix".to_string(), o => o.to_string() }, flow, if s("node").is_empty() { String::new() } else { format!(", step {}", s("node")) }),
        "webhook_create" => format!("make a webhook for {flow}{} (off until you switch it on; the address is shown only on your screen)", if args.get("signed").and_then(Value::as_bool) == Some(false) { ", NOT signed" } else { ", signed" }),
        "webhook_regenerate" => format!("make webhook {} new (its old address stops working at once)", s("id")),
        "webhook_enable" => format!("switch webhook {} ON (it starts its flow when called)", s("id")),
        "webhook_disable" => format!("switch webhook {} off", s("id")),
        "webhook_public" => format!("{} for webhook {} to take requests from outside the local network", if args.get("minutes").and_then(Value::as_u64).unwrap_or(0) == 0 { "STOP letting".to_string() } else { format!("LET, for {} minutes,", args.get("minutes").and_then(Value::as_u64).unwrap_or(0)) }, s("id")),
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
                let d = store::diff(&before, &f);
                let what = if d.is_empty() { String::new() } else { format!("\nWhat changed (steps added, changed, removed):\n{}", d.text()) };
                Ok(format!("Changed the draft of `{id}` (marked as the agent's; publishing it makes the next version): {}.{what}{}", done.join("; "), if issues.is_empty() { " No problems found.".into() } else { format!("\nProblems:\n{}", issues.join("\n")) }))
            }
            "flow_run" => {
                let id = self.flow_id(args, open)?;
                let at = if s(args, "at").is_empty() { None } else { Some(parse_when(s(args, "at"), now)?) };
                let mut req = StartRequest::new(&id).trigger("chat", AGENT);
                req.inputs = args.get("inputs").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
                req.at_ms = at;
                req.draft = args.get("draft").and_then(Value::as_bool).unwrap_or(false);
                // a missing required input is asked for with a form, not guessed
                if let Some(f) = self.store.published(&id).or_else(|| self.store.get_draft(&id)) {
                    let fields = askform::fields_for_missing(&f.inputs, &req.inputs);
                    if !fields.is_empty() {
                        let names: Vec<String> = fields.iter().filter_map(|x| x["name"].as_str().map(String::from)).collect();
                        return Err(format!("the flow needs {} before it can run. Call ask_form with title \"Inputs for {}\" and these fields, then call flow_run again with the answers as `inputs`: {}", names.join(", "), f.name, serde_json::to_string(&fields).unwrap_or_default()));
                    }
                }
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
                self.assist_after_publish(&id, v);
                let ups = self.engine.node_updates(&id);
                Ok(format!("Published `{id}` as version {v}.{}", if ups.is_empty() { String::new() } else { format!(" {} step(s) have runs on an older definition; flow_apply_to_runs can move them over.", ups.len()) }))
            }
            "flow_rollback" => {
                let id = self.flow_id(args, open)?;
                let to = args.get("version").and_then(Value::as_u64).ok_or("give `version`")? as u32;
                let v = self.store.rollback(&id, to, &self.who())?;
                self.host_audit("flow.rollback", &format!("{id} to v{to} as v{v} by {}", self.who()));
                self.assist_after_publish(&id, v);
                Ok(format!("Rolled `{id}` back to version {to}; published as version {v}."))
            }
            "flow_delete" => {
                let id = self.flow_id(args, open)?;
                let name = self.delete_flow(&id)?;
                Ok(format!("Deleted flow `{id}` (\"{name}\") with all its versions. Its runs stay in the history."))
            }
            "flow_prune_history" => {
                let rule = self.prune_rule(args);
                if args.get("dry_run").and_then(Value::as_bool).unwrap_or(false) {
                    return Ok(prune_text(&self.engine.retention_plan(Some(rule)), true));
                }
                let gone = self.engine.retention_apply(Some(rule));
                self.host_audit("flow.history.prune", &format!("{} run(s) removed by {} (keep {}, days {})", gone.len(), self.who(), rule.keep, rule.days));
                Ok(prune_text(&gone, false))
            }
            "flow_drop_version" => {
                let id = self.flow_id(args, open)?;
                let v = args.get("version").and_then(Value::as_u64).ok_or("give `version`")? as u32;
                self.drop_version(&id, v)?;
                Ok(format!("Dropped version {v} of `{id}`."))
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
            n if is_webhook_tool(n) => self.webhook_call(n, args, open),
            other => Err(format!("`{other}` is not a flows tool")),
        }
    }

    /// The webhook tools. The address and the shared secret go to the person's screen (a [`Reveal`](super::hookstore::Reveal)) and a
    /// note in the INBOX says so; the text returned here, which the model reads, never has them.
    fn webhook_call(&self, name: &str, args: &Value, open: Option<&SharedOpen>) -> Result<String, String> {
        use super::hookenv::HookOpts;
        use super::hookstore::{stash_reveal, Reveal};
        let id = s(args, "id");
        match name {
            "webhook_list" => {
                let flow = s(args, "flow");
                let v = self.hook_views(if flow.is_empty() { None } else { Some(flow) });
                let now = crate::clock::now_unix();
                Ok(if v.is_empty() { "There are no webhooks. webhook_create adds one to a flow.".into() } else { cut(v.iter().map(|h| h.line(now)).collect::<Vec<_>>().join("\n")) })
            }
            "webhook_create" => {
                let flow = self.flow_id(args, open)?;
                let strs = |k: &str| args.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default();
                let o = HookOpts {
                    signed: args.get("signed").and_then(Value::as_bool).unwrap_or(true),
                    methods: strs("methods"),
                    max_body: args.get("max_body").and_then(Value::as_u64).unwrap_or(0) as usize,
                    content_types: strs("content_types"),
                    rate_per_min: args.get("rate_per_min").and_then(Value::as_u64).unwrap_or(0).min(100_000) as u32,
                };
                let c = self.hook_create(&flow, &o, AGENT)?;
                self.reveal_to_screen(&c);
                stash_reveal(&self.user, Reveal { hook: c.id.clone(), flow: flow.clone(), path: c.path.clone(), secret: c.secret.clone() });
                Ok(format!("Made webhook `{}` on flow `{flow}`{}. It is OFF. Its secret address{} is shown on the person's screen only (the Webhook panel of the flow, key w): you cannot see it, so do not ask for it. Next: the person publishes the flow (flow_publish), then webhook_enable `{}`.", c.id, if o.signed { " (signed: HMAC-SHA256)" } else { " (not signed: the address alone is the protection)" }, if c.secret.is_some() { " and shared secret" } else { "" }, c.id))
            }
            "webhook_regenerate" => {
                let c = self.hook_regenerate(id, AGENT)?;
                self.reveal_to_screen(&c);
                stash_reveal(&self.user, Reveal { hook: c.id.clone(), flow: c.flow.clone(), path: c.path.clone(), secret: c.secret.clone() });
                Ok(format!("Made webhook `{}` new. The old address stopped working. The new one{} is shown on the person's screen only (key w in the flow).", c.id, if c.secret.is_some() { " and the new shared secret" } else { "" }))
            }
            "webhook_enable" => {
                self.hook_enable(id, AGENT)?;
                Ok(format!("Webhook `{id}` is ON: a call with the right address{} starts the published flow as the person.", if self.hook_views(None).iter().any(|h| h.id == id && h.signed) { " and signature" } else { "" }))
            }
            "webhook_disable" => {
                self.hook_disable(id, AGENT)?;
                Ok(format!("Webhook `{id}` is off. It answers 404 and any opening to the public is closed."))
            }
            "webhook_public" => {
                let minutes = args.get("minutes").and_then(Value::as_u64).ok_or("give `minutes` (0 closes it)")?;
                let until = self.hook_public(id, minutes, AGENT)?;
                Ok(if until == 0 { format!("Webhook `{id}` takes requests from the local network only.") } else { format!("Webhook `{id}` takes requests from outside the local network until {until} (Unix seconds). The firewall still decides what reaches the web port at all.") })
            }
            other => Err(format!("`{other}` is not a webhook tool")),
        }
    }

    /// A note in the INBOX that something is waiting on the screen; it never carries the address or the secret.
    fn reveal_to_screen(&self, c: &super::hookenv::Created) {
        use crate::inbox::{Kind, Link};
        let _ = self.svc.notify(&self.user, Kind::Agent, "A webhook address is waiting", &format!("Webhook {} on flow {}: open the flow in FLOWS and press w to see its address and secret once.", c.id, c.flow), Some(Link::new("flows", &c.flow)));
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
            "flow_delete" => {
                let id = self.flow_id(args, open).ok()?;
                let f = self.store.get_draft(&id)?;
                Some(format!("Flow `{id}` \"{}\": {} published version(s) and the draft are lost for good. Runs stay in the history.{}", f.name, self.store.versions(&id).len(), self.active_runs_text(&id, None)))
            }
            "flow_prune_history" => Some(prune_text(&self.engine.retention_plan(Some(self.prune_rule(args))), true)),
            "flow_drop_version" => {
                let id = self.flow_id(args, open).ok()?;
                let v = args.get("version")?.as_u64()? as u32;
                let f = self.store.get_draft(&id)?;
                let cur = self.store.meta_of(&id).published;
                Some(format!("Flow `{id}` \"{}\": version {v} is lost for good{}.{}", f.name, if cur == v { " (it is the CURRENT published version, so this will be refused)" } else { "" }, self.active_runs_text(&id, Some(v))))
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

    /// The history rule for a prune: what the person gave, else the tenant's `flows.quota`, else the defaults.
    fn prune_rule(&self, args: &Value) -> iyakki_flow::runstore::Retention {
        let q = super::super::cloudflows::quota::Quota::load(self.svc.dir(), None);
        iyakki_flow::runstore::Retention {
            keep: args.get("keep").and_then(Value::as_u64).map_or(q.history_keep, |k| k.clamp(1, 10_000) as usize),
            days: args.get("days").and_then(Value::as_u64).map_or(q.history_days, |d| d.min(3650)),
        }
    }

    /// Runs of the flow that are not finished (optionally only those on one version): ids, for the refusal and the card.
    fn active_run_ids(&self, id: &str, version: Option<u32>) -> Vec<String> {
        self.engine.summaries().iter().filter(|r| r.flow_id == id && !r.status.is_terminal() && version.is_none_or(|v| r.version == v)).map(|r| r.id.clone()).collect()
    }

    fn active_runs_text(&self, id: &str, version: Option<u32>) -> String {
        let a = self.active_run_ids(id, version);
        if a.is_empty() { String::new() } else { format!(" REFUSED while these runs are unfinished: {}.", a.join(", ")) }
    }

    /// Delete a flow after the person confirmed. Refused while it has runs that are running or scheduled. Audited by id and name.
    pub fn delete_flow(&self, id: &str) -> Result<String, String> {
        let f = self.store.get_draft(id).ok_or_else(|| format!("there is no flow `{id}`"))?;
        let a = self.active_run_ids(id, None);
        if !a.is_empty() {
            return Err(format!("`{id}` has runs that are not finished ({}): cancel them first, then delete the flow", a.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
        }
        self.store.delete(id)?;
        if super::last_flow(self).is_some_and(|(l, _)| l == id) {
            super::clear_last_flow(&self.svc, &self.user);
        }
        self.host_audit("flow.delete", &format!("{id} \"{}\" by {}", f.name, self.who()));
        Ok(f.name)
    }

    /// Drop one published version after the person confirmed. Never the current version; refused while an unfinished run uses it.
    pub fn drop_version(&self, id: &str, version: u32) -> Result<(), String> {
        let a = self.active_run_ids(id, Some(version));
        if !a.is_empty() {
            return Err(format!("version {version} of `{id}` is used by runs that are not finished ({}): wait for them or cancel them first", a.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
        }
        self.store.drop_version(id, version)?;
        self.host_audit("flow.version.drop", &format!("{id} v{version} by {}", self.who()));
        Ok(())
    }

    fn host_audit(&self, action: &str, detail: &str) {
        iyakki_flow::host::Host::audit(self.host.as_ref(), action, detail);
    }

    fn engine_now(&self) -> i64 {
        crate::clock::now_unix() * 1000
    }
}

/// The list of runs a prune removes (or would remove), at most 40 shown with the reason, the rest counted.
fn prune_text(items: &[iyakki_flow::runstore::PruneItem], dry: bool) -> String {
    if items.is_empty() {
        return "No finished run is old enough or beyond the limit: nothing to remove.".into();
    }
    let mut out = format!("{} {} run(s){}:\n", if dry { "Would remove" } else { "Removed" }, items.len(), if dry { " (nothing is removed yet)" } else { "" });
    for i in items.iter().take(40) {
        out.push_str(&format!("- {} of {}: {}\n", i.id, i.flow_id, i.reason));
    }
    if items.len() > 40 {
        out.push_str(&format!("... and {} more\n", items.len() - 40));
    }
    out
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

/// Tools that build an IYAKKI tile app or a web service. FLOWS does not offer them: "create" means a flow here.
pub fn is_app_creation_tool(name: &str) -> bool {
    matches!(name, "app_scaffold" | "app_preflight" | "app_bump_version" | "tile_scaffold" | "tile_validate" | "tile_test" | "tile_install_dev" | "tile_pack" | "tile_ship")
}

const APP_REFUSAL: &str = "App building is not available in FLOWS. Make a flow instead (flow_create), or tell the person to open the AI IDE if they really want an app.";

/// The rules for the model inside the FLOWS tile (kept short).
pub const PROMPT_RULES: &str = "\nYou work in FLOWS, the person's workflow tile (what is open is described below, refreshed with every message). Work on workflows first: prefer the flow_* tools (list, read, create, edit, run, explain a flow, its runs and failures) over every general tool. When a flow is OPEN ON SCREEN, requests to change, add to or update a flow apply to THAT flow: use flow_edit on it (leave `flow` out), step by step, and summarise what changed; call flow_create only when no flow is open or the person asks for a new one by name. Whatever the person asks you to create, make, build or automate here, the answer is a FLOW (flow_create): never an IYAKKI tile app, a web service, a Dockerfile or a deployment, and those tools are not offered. Only if the person explicitly asks for an app, say that apps are built in the AI IDE. When a flow needs a value you do not have (flow_run lists the missing required inputs), call ask_form with those fields instead of guessing or asking in prose; secrets go in a secret field, which stores them in the vault so you never see them. To build or change a flow use flow_create and flow_edit: they make DRAFTS, shown on the canvas at once, marked as yours, and the person publishes them. Never publish by yourself: ask with flow_publish only when the person says to, and it will ask them. Look before you change: flow_get first, then edit step by step (ids, ports and `{{ nodes.<id>.output.<field> }}` references must match). For \"why did run N fail\" call flow_explain_failure and quote its numbers; do not invent causes. Retry settings go in the node's `retry` (attempts, and backoff fixed, exponential or a per-attempt list of timeout and pause). Commands are a list of arguments, never a shell string. Credentials: never ask for, repeat or write one; use {{secret.NAME}} and, when the secret is missing, call vault_request so the person types it into their vault. You cannot approve your own fix: when a run needs the person, say so and point them to the card. A message that starts with FLOW FAILURE REQUEST asks you to fix a failed run of the open flow: read it with flow_explain_failure and flow_get, explain the cause in plain words, propose ONE fix with flow_edit on the open flow (the person approves it), never run, publish or roll back on your own, never ask for a credential, and when it says this is the last attempt and the cause is still unclear, stop and say why. Board and feature-row rules do not apply here.";

impl Tools for FlowTools {
    fn defs(&self) -> Vec<ToolDef> {
        // the flow tools lead; app creation is not offered here (a flow is the default thing to make in FLOWS)
        let mut v = defs(&self.env.policy);
        if !v.is_empty() {
            v.push(tool("ask_form", super::askform::def_description(), obj(json!({"title": {"type": "string"}, "fields": {"type": "array", "items": {"type": "object"}}}), &["fields"]), Risk::Safe, false));
        }
        v.extend(self.inner.defs().into_iter().filter(|d| !is_flow_tool(&d.spec.name) && d.spec.name != "ask_form" && !is_app_creation_tool(&d.spec.name)));
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == "ask_form" {
            return super::askform::parse(args).map(|f| format!("The form ({} fields) is shown to the person. Stop now: the filled values arrive as the next message.", f.fields.len()));
        }
        if is_app_creation_tool(name) {
            return Err(APP_REFUSAL.into());
        }
        if is_flow_tool(name) { self.env.call(name, args, Some(&self.open)) } else { self.inner.call(name, args) }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        if name == "ask_form" { return format!("ask you to fill in a form: {}", args.get("title").and_then(Value::as_str).unwrap_or("a few details").chars().take(100).collect::<String>()); }
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
        if is_flow_tool(name) { !matches!(name, "flow_publish" | "flow_rollback" | "flow_apply_to_runs" | "flow_set_heal_policy" | "flow_delete" | "flow_drop_version" | "flow_prune_history") } else { self.inner.approved_always(name) }
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
