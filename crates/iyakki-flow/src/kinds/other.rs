//! `browser` (steps through the browser automation), `agent` (an assistant step) and `script` (a small sandboxed script).

use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::*;
use crate::host::{AgentRequest, BrowserRequest, ScriptRequest};

pub fn register(r: &mut Registry) {
    r.register(Arc::new(BrowserKind));
    r.register(Arc::new(AgentKind));
    r.register(Arc::new(ScriptKind));
}

const STEP_KEYS: [&str; 14] = ["goto", "open", "click", "fill", "check", "select", "press", "wait_for", "wait", "expect", "assert", "screenshot", "snapshot_assert", "extract"];
const INTERACTING: [&str; 5] = ["click", "fill", "check", "select", "press"];

/// The step in the automation's own words (`open` is `goto`, `wait` is `wait_for`, `assert` is `expect`).
fn canonical_step(step: &Value) -> Value {
    let Some(o) = step.as_object() else { return step.clone() };
    let mut out = Map::new();
    for (k, v) in o {
        let nk = match k.as_str() {
            "open" => "goto",
            "wait" => "wait_for",
            "assert" => "expect",
            other => other,
        };
        let v = if nk == "wait_for" && v.is_number() { json!({"seconds": v}) } else { v.clone() };
        out.insert(nk.to_string(), v);
    }
    Value::Object(out)
}

struct BrowserKind;
impl NodeKind for BrowserKind {
    fn id(&self) -> &'static str {
        "browser"
    }
    fn label(&self) -> &'static str {
        "Browser"
    }
    fn blurb(&self) -> &'static str {
        "Opens pages, waits, clicks, fills, reads and checks, with the browser's consent rules."
    }
    fn category(&self) -> &'static str {
        "Run"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("steps", "Steps", FieldKind::Json, "[{\"open\": \"https://example.com\"}, {\"wait\": {\"text\": \"Example\"}}, {\"assert\": {\"text\": \"Domain\"}}, {\"screenshot\": \"home\"}]").required(),
            Field::new("allow_domains", "Domains", FieldKind::List, "the sites this step may open, one per line").required(),
            Field::new("level", "Level", FieldKind::Choice(&["read_only", "interactive"]), "read_only only looks; interactive may click and type after you approve each site (never granted automatically)").default(json!("read_only")),
            Field::new("target", "Where", FieldKind::Choice(&["headless", "ide"]), "a hidden helper or the IDE's own browser").default(json!("headless")),
            Field::new("extract", "Extract", FieldKind::Json, "{\"title\": \"title\", \"price\": {\"regex\": \"\\\\$(\\\\d+)\"}, \"page\": \"text\"}"),
        ]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let level = if s(c, "level").is_empty() { "read_only" } else { s(c, "level") };
        if !matches!(level, "read_only" | "interactive") {
            v.push(Issue::error(&node.id, "level is read_only or interactive"));
        }
        if list_of_strings(c.get("allow_domains").unwrap_or(&Value::Null)).is_empty() {
            v.push(Issue::error(&node.id, "name the sites this step may open (`allow_domains`)"));
        }
        match c.get("steps").and_then(Value::as_array) {
            Some(a) if !a.is_empty() => {
                for (i, st) in a.iter().enumerate() {
                    let keys: Vec<&String> = st.as_object().map(|o| o.keys().collect()).unwrap_or_default();
                    if keys.len() != 1 || !STEP_KEYS.contains(&keys[0].as_str()) {
                        v.push(Issue::error(&node.id, format!("step {} must be one of: {}", i + 1, STEP_KEYS.join(", "))));
                    } else if level == "read_only" && INTERACTING.contains(&canonical_step(st).as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default().as_str()) {
                        v.push(Issue::error(&node.id, format!("step {} interacts with the page: set level to `interactive` (you approve each site)", i + 1)));
                    }
                }
            }
            _ => v.push(Issue::error(&node.id, "give at least one step")),
        }
        v
    }
    fn idempotent(&self, node: &Node) -> bool {
        matches!(s(&node.config, "level"), "" | "read_only")
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let steps: Vec<Value> = cfg.get("steps").and_then(Value::as_array).map(|a| a.iter().filter(|s| s.get("extract").is_none()).map(canonical_step).collect()).unwrap_or_default();
        if steps.is_empty() {
            return Err(NodeError::config("give at least one step"));
        }
        let mut extract: Vec<(String, String, String)> = Vec::new();
        if let Some(Value::Object(o)) = cfg.get("extract") {
            for (name, spec) in o {
                match spec {
                    Value::String(kind) => extract.push((name.clone(), kind.clone(), String::new())),
                    Value::Object(m) => {
                        if let Some((k, v)) = m.iter().next() {
                            extract.push((name.clone(), k.clone(), crate::expr::text(v)));
                        }
                    }
                    _ => {}
                }
            }
        }
        let req = BrowserRequest {
            steps: Value::Array(steps),
            allow_domains: list_of_strings(cfg.get("allow_domains").unwrap_or(&Value::Null)),
            level: if s(cfg, "level").is_empty() { "read_only".into() } else { s(cfg, "level").to_string() },
            target: if s(cfg, "target").is_empty() { "headless".into() } else { s(cfg, "target").to_string() },
            timeout_ms: cx.timeout_ms,
            extract,
            run_id: cx.run_id.to_string(),
            node_id: cx.node_id.to_string(),
        };
        cx.log(format!("browser: {} steps on {} ({}, {})", req.steps.as_array().map_or(0, Vec::len), req.allow_domains.join(", "), req.level, req.target));
        let r = cx.host.browser(&req, cx.cancel)?;
        for l in r.report.lines().take(40) {
            cx.log(l);
        }
        let mut out = Map::new();
        out.insert("report".into(), json!(r.report));
        out.insert("url".into(), json!(r.url));
        out.insert("screenshots".into(), json!(r.screenshots));
        for (k, v) in r.extracted {
            if !matches!(k.as_str(), "report" | "url" | "screenshots") {
                out.insert(k, v);
            }
        }
        Ok(NodeOutput { value: Value::Object(out), port: "out".into(), wait: None, artifacts: r.screenshots })
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        if e.navigation || e.timed_out {
            ErrClass::Retryable
        } else {
            default_class(e)
        }
    }
    fn describe(&self, node: &Node) -> String {
        format!("browse {}", list_of_strings(node.config.get("allow_domains").unwrap_or(&Value::Null)).join(", "))
    }
}

struct AgentKind;
impl NodeKind for AgentKind {
    fn id(&self) -> &'static str {
        "agent"
    }
    fn label(&self) -> &'static str {
        "Agent"
    }
    fn blurb(&self) -> &'static str {
        "Asks the assistant (or an external agent) with a prompt, a tool allow-list and a budget."
    }
    fn category(&self) -> &'static str {
        "Run"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("prompt", "Prompt", FieldKind::Multiline, "may use {{ templates }}").required(),
            Field::new("images", "Pictures", FieldKind::List, "picture files to show the model, one per line, e.g. {{ nodes.shot.output.path }} (needs a model that can look at pictures)"),
            Field::new("tools", "Tools it may use", FieldKind::List, "names, one per line; empty means none"),
            Field::new("output_schema", "Output schema", FieldKind::Json, "{\"required\": [\"summary\"]}: the answer must be JSON with these keys"),
            Field::new("agent", "Agent", FieldKind::Text, "empty is the configured model; or an external agent id").default(json!("")),
            Field::new("max_tokens", "Most tokens", FieldKind::Number, "").default(json!(2000)),
            Field::new("max_cents", "Most cost (cents)", FieldKind::Number, "").default(json!(20)),
        ]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        if s(&node.config, "prompt").trim().is_empty() {
            v.push(Issue::error(&node.id, "write a prompt"));
        }
        if list_of_strings(node.config.get("images").unwrap_or(&Value::Null)).len() > 4 {
            v.push(Issue::error(&node.id, "at most 4 pictures in one step"));
        }
        if let Some(sch) = node.config.get("output_schema") {
            if !sch.is_object() {
                v.push(Issue::error(&node.id, "the output schema is a JSON object"));
            }
        }
        if !list_of_strings(node.config.get("tools").unwrap_or(&Value::Null)).is_empty() {
            v.push(Issue::error(&node.id, "tools in an agent step are planned: leave the tool list empty for now"));
        }
        if !s(&node.config, "agent").is_empty() {
            v.push(Issue::error(&node.id, "an external agent in a flow step is planned: leave `agent` empty to use the configured model"));
        }
        v
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let req = AgentRequest {
            prompt: s(cfg, "prompt").to_string(),
            images: list_of_strings(cfg.get("images").unwrap_or(&Value::Null)).into_iter().filter(|p| !p.trim().is_empty()).collect(),
            tools: list_of_strings(cfg.get("tools").unwrap_or(&Value::Null)),
            output_schema: cfg.get("output_schema").cloned().unwrap_or(Value::Null),
            agent: s(cfg, "agent").to_string(),
            max_tokens: u(cfg, "max_tokens").unwrap_or(2000).min(32_000),
            max_cents: u(cfg, "max_cents").unwrap_or(20).min(1000),
            timeout_ms: cx.timeout_ms,
            run_id: cx.run_id.to_string(),
            node_id: cx.node_id.to_string(),
        };
        cx.log(format!("agent step ({} tools, up to {} tokens, {} cents)", req.tools.len(), req.max_tokens, req.max_cents));
        let r = cx.host.agent(&req, cx.cancel)?;
        let mut out = Map::new();
        out.insert("text".into(), json!(r.text));
        out.insert("cents".into(), json!(r.cents));
        if req.output_schema.is_object() {
            let t = r.text.as_str();
            let parsed = t.find(['{', '[']).and_then(|a| t.rfind(['}', ']']).map(|b| (a, b))).and_then(|(a, b)| serde_json::from_str::<Value>(&t[a..=b]).ok());
            let Some(j) = parsed else {
                return Err(NodeError { message: "the answer is not JSON, but an output schema was given".into(), transient: Some(true), ..Default::default() });
            };
            for k in req.output_schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if j.get(k).is_none() {
                    return Err(NodeError { message: format!("the answer has no `{k}`, which the output schema requires"), transient: Some(true), ..Default::default() });
                }
            }
            if let Value::Object(m) = &j {
                for (k, v) in m {
                    if !matches!(k.as_str(), "text" | "cents") {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            out.insert("json".into(), j);
        }
        Ok(NodeOutput::out(Value::Object(out)))
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        default_class(e)
    }
}

struct ScriptKind;
impl NodeKind for ScriptKind {
    fn id(&self) -> &'static str {
        "script"
    }
    fn label(&self) -> &'static str {
        "Script"
    }
    fn blurb(&self) -> &'static str {
        "A small sandboxed script (JavaScript, or an installed WASM tool): no network, no files, time and memory capped."
    }
    fn category(&self) -> &'static str {
        "Run"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("lang", "Language", FieldKind::Choice(&["js", "wasm"]), "").default(json!("js")),
            Field::new("source", "Script", FieldKind::Multiline, "JavaScript: `args` is the input; `return` the result").required(),
            Field::new("package", "WASM tool", FieldKind::Text, "for `wasm`: the name of an installed tool package"),
            Field::new("args", "Arguments", FieldKind::Json, "may use {{ templates }}"),
        ]
    }
    fn raw_fields(&self) -> &'static [&'static str] {
        &["source"]
    }
    fn idempotent(&self, _n: &Node) -> bool {
        true
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let lang = if s(c, "lang").is_empty() { "js" } else { s(c, "lang") };
        match lang {
            "js" if s(c, "source").trim().is_empty() => vec![Issue::error(&node.id, "write the script")],
            "wasm" if s(c, "package").trim().is_empty() => vec![Issue::error(&node.id, "name the WASM tool package")],
            "js" | "wasm" => vec![],
            other => vec![Issue::error(&node.id, format!("`{other}` is not a script language (js, wasm)"))],
        }
    }
    fn run(&self, cx: &NodeCx, _n: &Node, cfg: &Value) -> NodeResult {
        let req = ScriptRequest { lang: if s(cfg, "lang").is_empty() { "js".into() } else { s(cfg, "lang").to_string() }, source: s(cfg, "source").to_string(), package: s(cfg, "package").to_string(), args: cfg.get("args").cloned().unwrap_or(Value::Null), timeout_ms: cx.timeout_ms };
        let v = cx.host.script(&req, cx.cancel)?;
        Ok(NodeOutput::out(json!({"result": v})))
    }
}
