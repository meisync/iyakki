//! `cli` (one program, as an argument array), `grpc` (through the `grpcurl` program) and `thrift` (over HTTP, JSON protocol).

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::heal::version_ge;
use crate::host::{ExecRequest, ExecResult};
use crate::retry::grpc_code_retryable;

pub fn register(r: &mut Registry) {
    r.register(Arc::new(CliKind));
    r.register(Arc::new(GrpcKind));
    r.register(Arc::new(ThriftKind));
}

const SHELLS: [&str; 13] = ["sh", "bash", "zsh", "dash", "ash", "ksh", "fish", "csh", "tcsh", "cmd", "cmd.exe", "powershell", "pwsh"];

fn argv_of(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(crate::expr::text).collect(),
        _ => vec![],
    }
}

fn shell_name(prog: &str) -> bool {
    let base = prog.rsplit('/').next().unwrap_or(prog).to_ascii_lowercase();
    SHELLS.contains(&base.as_str())
}

fn cwd_ok(c: &str) -> bool {
    !c.starts_with('/') && !c.starts_with('\\') && !c.split(['/', '\\']).any(|p| p == "..") && !c.contains(':') && !c.chars().any(char::is_control)
}

fn env_of(cfg: &Value) -> Vec<(String, String)> {
    match cfg.get("env") {
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), crate::expr::text(v))).collect(),
        _ => vec![],
    }
}

/// Check that the CLI a node needs is there (and new enough), then run the command by the node's runner.
fn execute(cx: &NodeCx, cfg: &Value, argv: Vec<String>, cli: Option<(&str, &str)>, secrets: bool) -> Result<ExecResult, NodeError> {
    if let Some((id, min)) = cli.filter(|c| !c.0.is_empty()) {
        match cx.host.cli_info(id) {
            Some(info) if !info.installed => {
                return Err(NodeError { message: format!("{id} is not installed (this step needs {id}{})", if min.is_empty() { String::new() } else { format!(" {min} or newer") }), missing: true, needs_cli: Some((id.into(), min.into(), String::new())), ..Default::default() });
            }
            Some(info) if !min.is_empty() && !info.version.is_empty() && !version_ge(&info.version, min) => {
                return Err(NodeError { message: format!("{id} {} is older than the {min} this step needs", info.version), needs_cli: Some((id.into(), min.into(), info.version)), ..Default::default() });
            }
            _ => {}
        }
    }
    if let Some(p) = argv.first() {
        if shell_name(p) {
            return Err(NodeError::config("shells are not allowed: run the program directly with its arguments"));
        }
    }
    let cwd = s(cfg, "cwd").to_string();
    if !cwd_ok(&cwd) {
        return Err(NodeError::config("`cwd` must be a folder inside the flow's workspace (no `..`, no absolute path)"));
    }
    let req = ExecRequest { argv: argv.clone(), cwd, env: env_of(cfg), timeout_ms: cx.timeout_ms, runner: cx.runner, image: if s(cfg, "image").is_empty() { cx.flow.defaults.sandbox.image.clone() } else { s(cfg, "image").to_string() }, namespace: cx.flow.defaults.sandbox.namespace.clone(), flow_id: cx.flow.id.clone(), run_id: cx.run_id.to_string(), node_id: cx.node_id.to_string(), secrets };
    cx.log(format!("$ {} (attempt {}, limit {} ms, {})", argv.join(" "), cx.attempt, cx.timeout_ms, cx.runner.label()));
    let r = cx.host.exec(&req, cx.cancel)?;
    if !r.ran_in.is_empty() {
        cx.log(format!("ran in {}", r.ran_in));
    }
    for l in r.stdout.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
        cx.log(format!("| {l}"));
    }
    for l in r.stderr.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
        cx.log(format!("! {l}"));
    }
    Ok(r)
}

fn combined(r: &ExecResult) -> String {
    let t = if r.stderr.is_empty() { r.stdout.clone() } else if r.stdout.is_empty() { r.stderr.clone() } else { format!("{}\n{}", r.stdout, r.stderr) };
    t.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect()
}

fn last_line(t: &str) -> String {
    t.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().chars().take(200).collect()
}

struct CliKind;
impl NodeKind for CliKind {
    fn id(&self) -> &'static str {
        "cli"
    }
    fn label(&self) -> &'static str {
        "Command"
    }
    fn blurb(&self) -> &'static str {
        "Runs one program (an argument list, never a shell) in the flow's workspace."
    }
    fn category(&self) -> &'static str {
        "Run"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("command", "Command", FieldKind::List, "one argument per line: the program first").required(),
            Field::new("cwd", "Folder", FieldKind::Text, "inside the flow's workspace; empty is its root"),
            Field::new("env", "Environment", FieldKind::Json, "NAME: value; a secret as {{secret.NAME}}"),
            Field::new("parse", "Read the output as", FieldKind::Choice(&["text", "json", "lines", "regex"]), "").default(json!("text")),
            Field::new("pattern", "Pattern", FieldKind::Text, "for `regex`: the groups become `data.1`, `data.2`"),
            Field::new("cli", "Needs the CLI", FieldKind::Json, "{\"id\": \"kubectl\", \"min_version\": \"1.28\"} from the CLI tile's catalog"),
            Field::new("retry_exit_codes", "Retry on exit codes", FieldKind::Json, "[75, 111]"),
            Field::new("retry_patterns", "Retry when the output has", FieldKind::List, "one text per line (case does not matter)"),
            Field::new("image", "Container image", FieldKind::Text, "for the sandbox runner; empty is the default tools image"),
            Field::new("read_only", "Changes nothing", FieldKind::Bool, "lets a restart and a heal test run it again safely").default(json!(false)),
        ]
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["env"]
    }
    fn sandboxable(&self) -> bool {
        true
    }
    fn idempotent(&self, node: &Node) -> bool {
        b(&node.config, "read_only", false)
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        match c.get("command") {
            Some(Value::Array(a)) if !a.is_empty() && a.iter().all(|x| x.is_string()) => {
                let p = a[0].as_str().unwrap_or("");
                if p.trim().is_empty() {
                    v.push(Issue::error(&node.id, "the program name is empty"));
                } else if shell_name(p) {
                    v.push(Issue::error(&node.id, "shells are not allowed: run the program directly with its arguments"));
                } else if p.contains(' ') && !p.contains("{{") {
                    v.push(Issue::error(&node.id, "the first item is the program only; put each argument in its own item"));
                }
            }
            Some(Value::String(_)) => v.push(Issue::error(&node.id, "`command` is a list: [\"program\", \"argument\"], not a text for a shell")),
            _ => v.push(Issue::error(&node.id, "give a command: a list with the program first")),
        }
        if !cwd_ok(s(c, "cwd")) {
            v.push(Issue::error(&node.id, "`cwd` must stay inside the flow's workspace"));
        }
        if c.get("stdin").is_some() {
            v.push(Issue::error(&node.id, "stdin is not supported yet: put the data in a file or an argument"));
        }
        if !matches!(s(c, "parse"), "" | "text" | "json" | "lines" | "regex") {
            v.push(Issue::error(&node.id, "parse is text, json, lines or regex"));
        }
        if s(c, "parse") == "regex" && crate::minire::Re::new(s(c, "pattern")).is_err() {
            v.push(Issue::error(&node.id, "the pattern is not a valid regular expression"));
        }
        if let Some(cli) = c.get("cli") {
            if s(cli, "id").is_empty() {
                v.push(Issue::error(&node.id, "`cli` needs an `id` from the catalog"));
            }
        }
        v
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let argv = argv_of(cfg.get("command").unwrap_or(&Value::Null));
        if argv.is_empty() || argv[0].trim().is_empty() {
            return Err(NodeError::config("give a command: a list with the program first"));
        }
        if cfg.get("stdin").is_some() {
            return Err(NodeError::config("stdin is not supported yet"));
        }
        let cli = cfg.get("cli").map(|c| (s(c, "id"), s(c, "min_version")));
        let r = execute(cx, cfg, argv.clone(), cli, node.config.to_string().contains("{{secret."))?;
        let out = combined(&r);
        if r.missing {
            return Err(NodeError { message: format!("`{}` was not found: it is not installed on this system. Use the HTTP request step for web calls, or add the program first", argv[0]), missing: true, output: out, ..Default::default() });
        }
        if r.timed_out {
            return Err(NodeError { message: format!("`{}` took longer than {} ms and was stopped", argv[0], cx.timeout_ms), timed_out: true, output: out, ..Default::default() });
        }
        if r.code != Some(0) {
            return Err(NodeError { message: format!("`{}` exited with code {}: {}", argv[0], r.code.map_or("?".to_string(), |c| c.to_string()), last_line(&if r.stderr.trim().is_empty() { r.stdout.clone() } else { r.stderr.clone() })), exit_code: r.code, output: out, ..Default::default() });
        }
        let data = parse_output(s(cfg, "parse"), s(cfg, "pattern"), &r.stdout)?;
        Ok(NodeOutput::out(json!({"code": 0, "stdout": r.stdout.chars().take(64 * 1024).collect::<String>(), "stderr": r.stderr.chars().take(8 * 1024).collect::<String>(), "data": data, "ran_in": r.ran_in})))
    }
    fn classify_error(&self, e: &NodeError, node: &Node) -> ErrClass {
        if e.cancelled || e.config || e.missing || e.needs_cli.is_some() {
            return ErrClass::Permanent;
        }
        if e.timed_out {
            return ErrClass::Retryable;
        }
        if let Some(code) = e.exit_code {
            if node.config.get("retry_exit_codes").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x.as_i64() == Some(code as i64))) {
                return ErrClass::Retryable;
            }
        }
        let low = e.output.to_ascii_lowercase();
        if list_of_strings(node.config.get("retry_patterns").unwrap_or(&Value::Null)).iter().any(|p| !p.is_empty() && low.contains(&p.to_ascii_lowercase())) {
            return ErrClass::Retryable;
        }
        default_class(e)
    }
    fn describe(&self, node: &Node) -> String {
        format!("run {}", argv_of(node.config.get("command").unwrap_or(&Value::Null)).join(" "))
    }
}

// ------------------------------------------------------------------------------------------------ grpc

/// `DeadlineExceeded` to `DEADLINE_EXCEEDED`.
pub fn snake_upper(camel: &str) -> String {
    let mut out = String::new();
    for (i, c) in camel.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// The status of a failed grpcurl run: from its `Code: Unavailable` line, from a failure to connect, or from a stop by time.
pub fn grpc_status(r: &ExecResult) -> Option<(String, String)> {
    if r.timed_out {
        return Some(("DEADLINE_EXCEEDED".into(), "the call took longer than the deadline".into()));
    }
    let text = if r.stderr.is_empty() { &r.stdout } else { &r.stderr };
    let mut code = None;
    let mut message = String::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(c) = t.strip_prefix("Code:") {
            code = Some(snake_upper(c.trim()));
        } else if let Some(m) = t.strip_prefix("Message:") {
            message = m.trim().to_string();
        }
    }
    if let Some(c) = code {
        return Some((c, message));
    }
    let low = text.to_ascii_lowercase();
    if low.contains("failed to dial") || low.contains("connection refused") || low.contains("connection error") || low.contains("no such host") {
        return Some(("UNAVAILABLE".into(), last_line(text)));
    }
    if low.contains("deadline exceeded") || low.contains("context deadline") {
        return Some(("DEADLINE_EXCEEDED".into(), last_line(text)));
    }
    None
}

struct GrpcKind;
impl NodeKind for GrpcKind {
    fn id(&self) -> &'static str {
        "grpc"
    }
    fn label(&self) -> &'static str {
        "gRPC"
    }
    fn blurb(&self) -> &'static str {
        "Calls a gRPC method through the grpcurl program (JSON in, JSON out)."
    }
    fn category(&self) -> &'static str {
        "Call"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("address", "Address", FieldKind::Text, "host:port").required(),
            Field::new("method", "Method", FieldKind::Text, "package.Service/Method").required(),
            Field::new("data", "Request (JSON)", FieldKind::Json, "the request message").default(json!({})),
            Field::new("plaintext", "No TLS", FieldKind::Bool, "plain HTTP/2 (for a local server)").default(json!(false)),
            Field::new("proto_path", "Proto file", FieldKind::Text, "leave empty to use server reflection"),
            Field::new("import_paths", "Proto folders", FieldKind::List, "one per line"),
            Field::new("metadata", "Metadata", FieldKind::Json, "headers: name: value; a secret as {{secret.NAME}}"),
            Field::new("deadline_ms", "Deadline (ms)", FieldKind::Number, "default: the step's timeout"),
            Field::new("cli", "grpcurl version", FieldKind::Json, "{\"min_version\": \"1.8.0\"}"),
            Field::new("image", "Container image", FieldKind::Text, "for the sandbox runner"),
            Field::new("read_only", "Changes nothing", FieldKind::Bool, "").default(json!(false)),
        ]
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["metadata"]
    }
    fn sandboxable(&self) -> bool {
        true
    }
    fn idempotent(&self, node: &Node) -> bool {
        b(&node.config, "read_only", false)
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let a = s(c, "address");
        if a.is_empty() || (!a.contains("{{") && (!a.contains(':') || a.contains(' ') || a.contains("://"))) {
            v.push(Issue::error(&node.id, "the address is host:port"));
        }
        let m = s(c, "method");
        if m.is_empty() || (!m.contains("{{") && (m.contains(' ') || !(m.contains('/') || m.contains('.')))) {
            v.push(Issue::error(&node.id, "the method looks like package.Service/Method"));
        }
        for (k, val) in headers_like(c.get("metadata")) {
            if credential_header(&k) && !val.contains("{{secret.") && !val.is_empty() {
                v.push(Issue::error(&node.id, format!("metadata `{k}` looks like a credential: write it as {{{{secret.NAME}}}}")));
            }
        }
        v
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let (addr, method) = (s(cfg, "address"), s(cfg, "method"));
        if addr.is_empty() || method.is_empty() {
            return Err(NodeError::config("give the address and the method"));
        }
        let deadline = u(cfg, "deadline_ms").unwrap_or(cx.timeout_ms).max(1);
        let mut argv = vec!["grpcurl".to_string()];
        if b(cfg, "plaintext", false) {
            argv.push("-plaintext".into());
        }
        argv.push("-max-time".into());
        argv.push(format!("{}", (deadline as f64 / 1000.0).max(0.001)));
        for (k, v) in headers_like(cfg.get("metadata")) {
            argv.push("-H".into());
            argv.push(format!("{k}: {v}"));
        }
        for p in list_of_strings(cfg.get("import_paths").unwrap_or(&Value::Null)) {
            argv.push("-import-path".into());
            argv.push(p);
        }
        if !s(cfg, "proto_path").is_empty() {
            argv.push("-proto".into());
            argv.push(s(cfg, "proto_path").to_string());
        }
        let data = match cfg.get("data") {
            None | Some(Value::Null) => "{}".to_string(),
            Some(Value::String(t)) => t.clone(),
            Some(other) => other.to_string(),
        };
        argv.push("-d".into());
        argv.push(data);
        argv.push(addr.to_string());
        argv.push(method.to_string());
        let min = cfg.get("cli").map(|c| s(c, "min_version")).unwrap_or("");
        let r = execute(cx, cfg, argv, Some(("grpcurl", min)), node.config.to_string().contains("{{secret."))?;
        if r.missing {
            return Err(NodeError { message: "grpcurl was not found".into(), missing: true, needs_cli: Some(("grpcurl".into(), min.into(), String::new())), ..Default::default() });
        }
        if r.timed_out || r.code != Some(0) {
            let (code, msg) = grpc_status(&r).unwrap_or_else(|| ("UNKNOWN".into(), last_line(&combined(&r))));
            return Err(NodeError { message: format!("gRPC {code}: {msg}"), grpc_code: Some(code.clone()), timed_out: r.timed_out || code == "DEADLINE_EXCEEDED", connection: code == "UNAVAILABLE", exit_code: r.code, output: combined(&r), ..Default::default() });
        }
        let parsed = if r.stdout.trim().is_empty() { json!({}) } else { parse_output("json", "", &r.stdout)? };
        Ok(NodeOutput::out(json!({"code": "OK", "data": parsed, "stdout": r.stdout.chars().take(64 * 1024).collect::<String>(), "ran_in": r.ran_in})))
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        if e.cancelled || e.config || e.missing || e.needs_cli.is_some() {
            return ErrClass::Permanent;
        }
        match &e.grpc_code {
            Some(c) => {
                if grpc_code_retryable(c) {
                    ErrClass::Retryable
                } else {
                    ErrClass::Permanent
                }
            }
            None => default_class(e),
        }
    }
    fn describe(&self, node: &Node) -> String {
        format!("gRPC {} on {}", s(&node.config, "method"), s(&node.config, "address"))
    }
}

fn headers_like(v: Option<&Value>) -> Vec<(String, String)> {
    match v {
        Some(Value::Object(o)) => o.iter().map(|(k, x)| (k.clone(), crate::expr::text(x))).collect(),
        _ => vec![],
    }
}

// ------------------------------------------------------------------------------------------------ thrift

/// A Thrift call over HTTP with the JSON protocol (`crate::thrift`): the node names the method and gives every argument with its field
/// number, type and value; the answer is turned into JSON without a description of the service.
struct ThriftKind;
impl NodeKind for ThriftKind {
    fn id(&self) -> &'static str {
        "thrift"
    }
    fn label(&self) -> &'static str {
        "Thrift"
    }
    fn blurb(&self) -> &'static str {
        "Calls a Thrift service over HTTP (JSON protocol): the method and its arguments, each with a field number, a type and a value."
    }
    fn category(&self) -> &'static str {
        "Call"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("endpoint", "Address", FieldKind::Text, "http:// or https:// address of the service").required(),
            Field::new("service", "Service", FieldKind::Text, "only for a server that expects Service:method (multiplexed)"),
            Field::new("method", "Method", FieldKind::Text, "").required(),
            Field::new("args", "Arguments", FieldKind::Json, "[{\"id\": 1, \"type\": \"string\", \"value\": \"x\"}, {\"id\": 2, \"type\": {\"list\": \"i32\"}, \"value\": [1, 2]}]; types: bool, byte, i16, i32, i64, double, string, binary, {list}, {set}, {map: [K, V]}, {struct: [fields]}"),
            Field::new("oneway", "One way", FieldKind::Bool, "send and do not wait for an answer").default(json!(false)),
            Field::new("headers", "Headers", FieldKind::Json, "name: value; a credential only as {{secret.NAME}}"),
            Field::new("tls", "TLS", FieldKind::Json, "{\"ca_file\": \"/path/ca.pem\", \"insecure\": false}"),
            Field::new("protocol", "Protocol", FieldKind::Choice(&["json-http"]), "the JSON protocol over HTTP; binary and compact are not offered").default(json!("json-http")),
        ]
    }
    fn secret_fields(&self) -> &'static [&'static str] {
        &["headers"]
    }
    fn validate(&self, node: &Node, _f: &Flow) -> Vec<Issue> {
        let c = &node.config;
        let mut v = Vec::new();
        let url = s(c, "endpoint");
        if url.is_empty() {
            v.push(Issue::error(&node.id, "give the address of the service"));
        } else if !url.contains("{{") && !(url.starts_with("http://") || url.starts_with("https://")) {
            v.push(Issue::error(&node.id, "the address must start with http:// or https:// (only Thrift over HTTP is available)"));
        }
        let m = s(c, "method");
        if m.is_empty() {
            v.push(Issue::error(&node.id, "give the method"));
        } else if !m.contains("{{") && !m.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            v.push(Issue::error(&node.id, "the method name has letters, digits and underscores only"));
        }
        if !s(c, "protocol").is_empty() && s(c, "protocol") != "json-http" {
            v.push(Issue::error(&node.id, format!("protocol `{}` is not available; only json-http", s(c, "protocol"))));
        }
        if let Some(a) = c.get("args") {
            if let Err(e) = crate::thrift::parse_args(a) {
                v.push(Issue::error(&node.id, e));
            }
        }
        if let Some(Value::Object(h)) = c.get("headers") {
            for (k, val) in h {
                let val = crate::expr::text(val);
                if credential_header(k) && !val.is_empty() && !val.contains("{{secret.") {
                    v.push(Issue::error(&node.id, format!("header `{k}` looks like a credential: write it as {{{{secret.NAME}}}} and keep the value in your vault")));
                }
            }
        }
        v
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let url = s(cfg, "endpoint").trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(NodeError::config("the address must start with http:// or https://"));
        }
        let args = crate::thrift::parse_args(cfg.get("args").unwrap_or(&Value::Null)).map_err(NodeError::config)?;
        let oneway = b(cfg, "oneway", false);
        let seq = 1;
        let body = crate::thrift::encode_call(s(cfg, "service"), s(cfg, "method"), seq, oneway, &args).map_err(NodeError::config)?;
        let mut headers: Vec<(String, String)> = match cfg.get("headers") {
            Some(Value::Object(o)) => o.iter().map(|(k, x)| (k.clone(), crate::expr::text(x))).collect(),
            _ => vec![],
        };
        headers.push(("Content-Type".into(), "application/x-thrift".into()));
        headers.push(("Accept".into(), "application/x-thrift".into()));
        let tls = cfg.get("tls").map(|t| crate::host::TlsOpts { ca_file: s(t, "ca_file").to_string(), insecure: t.get("insecure").and_then(Value::as_bool).unwrap_or(false) }).unwrap_or_default();
        let req = crate::host::HttpRequest { method: "POST".into(), url: url.clone(), headers, body: body.into_bytes(), timeout_ms: cx.timeout_ms, tls, follow_redirects: false, run_id: cx.run_id.to_string(), node_id: cx.node_id.to_string() };
        cx.log(format!("Thrift {} on {url} (attempt {}, limit {} ms)", s(cfg, "method"), cx.attempt, cx.timeout_ms));
        let resp = cx.host.http(&req, cx.cancel)?;
        cx.log(format!("-> {} in {} ms, {} bytes", resp.status, resp.elapsed_ms, resp.body.len()));
        if !(200..300).contains(&resp.status) {
            return Err(NodeError { message: format!("HTTP {}: {}", resp.status, preview(&json!(resp.body.trim()), 200)), http_status: Some(resp.status), output: preview(&json!(resp.body), 2000), ..Default::default() });
        }
        let _ = node;
        if oneway {
            return Ok(NodeOutput::out(json!({"status": resp.status, "result": Value::Null, "fields": {}})));
        }
        match crate::thrift::decode_reply(&resp.body, s(cfg, "method"), seq).map_err(|e| NodeError { message: e, output: preview(&json!(resp.body), 2000), ..Default::default() })? {
            crate::thrift::Reply::Return { result, fields } => Ok(NodeOutput::out(json!({"status": resp.status, "result": result, "fields": fields}))),
            crate::thrift::Reply::Declared { field, value } => Err(NodeError { message: format!("the service threw its exception number {field}: {}", preview(&value, 300)), output: preview(&value, 2000), ..Default::default() }),
            crate::thrift::Reply::Application { message, kind } => Err(NodeError { message: format!("the server failed ({}): {message}", crate::thrift::exception_name(kind)), output: message, ..Default::default() }),
        }
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        if e.cancelled || e.config {
            return ErrClass::Permanent;
        }
        match e.http_status {
            Some(s) if crate::retry::http_status_retryable(s) => ErrClass::Retryable,
            Some(_) => ErrClass::Permanent,
            None => default_class(e),
        }
    }
    fn describe(&self, node: &Node) -> String {
        format!("{} on {}", s(&node.config, "method"), preview(&json!(s(&node.config, "endpoint")), 50))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_and_messages() {
        assert_eq!(snake_upper("DeadlineExceeded"), "DEADLINE_EXCEEDED");
        assert_eq!(snake_upper("Unavailable"), "UNAVAILABLE");
        assert_eq!(snake_upper("ResourceExhausted"), "RESOURCE_EXHAUSTED");
        let r = ExecResult { code: Some(64), stderr: "ERROR:\n  Code: Unavailable\n  Message: connection error: desc = \"x\"\n".into(), ..Default::default() };
        assert_eq!(grpc_status(&r).unwrap().0, "UNAVAILABLE");
        let r = ExecResult { code: Some(1), stderr: "Failed to dial target host \"x:1\": dial tcp 127.0.0.1:1: connect: connection refused".into(), ..Default::default() };
        assert_eq!(grpc_status(&r).unwrap().0, "UNAVAILABLE");
        let r = ExecResult { timed_out: true, ..Default::default() };
        assert_eq!(grpc_status(&r).unwrap().0, "DEADLINE_EXCEEDED");
        assert!(grpc_status(&ExecResult { code: Some(2), stderr: "weird".into(), ..Default::default() }).is_none());
    }

    #[test]
    fn shells_and_folders_are_refused() {
        assert!(shell_name("/bin/sh") && shell_name("bash") && shell_name("PowerShell") && !shell_name("kubectl"));
        assert!(cwd_ok("") && cwd_ok("a/b") && !cwd_ok("../x") && !cwd_ok("/etc") && !cwd_ok("a/../../b") && !cwd_ok("C:x"));
    }
}
