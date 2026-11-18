//! Repairs for tool calls written by small or odd models.
//!
//! Weak models call tools in many almost-right ways: JSON inside a markdown fence, single quotes, a trailing comma, the
//! arguments as a string instead of an object, `web.search` or `Browser-Open` for `web_search` / `browser_open`, a
//! `{"name": ..., "arguments": ...}` wrapper printed as plain text because the model has no native tool calling, numbers
//! written as strings. This module turns those into a proper [`ToolCall`], checks the arguments against the tool's schema and
//! writes an error that tells the model exactly what was wrong and what is valid. It also holds the plain-text protocol (a
//! fenced `tool` block) used for models whose server refuses native tools.

use serde_json::{json, Map, Value};

use crate::chat::{ToolCall, ToolSpec};

// ---------------------------------------------------------------------------------------------------------- loose JSON

/// Text with a markdown fence (```json ... ```) taken off, if it has one.
pub fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let body = rest.split_once('\n').map_or(rest, |(_, b)| b);
        return body.rsplit_once("```").map_or(body, |(b, _)| b).trim().to_string();
    }
    t.to_string()
}

/// The first balanced `{...}` or `[...]` in `s` (strings of either quote kind are skipped over).
fn first_balanced(s: &str) -> Option<&str> {
    let start = s.find(['{', '['])?;
    let (mut depth, mut quote, mut esc) = (0i32, None::<char>, false);
    for (i, c) in s[start..].char_indices() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Rewrite almost-JSON as JSON: single quotes, bare keys, trailing commas, Python literals, curly quotes, a cut-off end.
fn normalize(s: &str) -> String {
    let s = s.replace(['\u{201c}', '\u{201d}'], "\"").replace(['\u{2018}', '\u{2019}'], "'");
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut stack: Vec<char> = Vec::new();
    let (mut quote, mut esc) = (None::<char>, false);
    let mut i = 0;
    // the last significant character written outside a string
    let mut last = ' ';
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if esc {
                esc = false;
                out.push(c);
            } else if c == '\\' {
                esc = true;
                out.push(c);
            } else if c == q {
                quote = None;
                out.push('"');
            } else if c == '"' {
                out.push_str("\\\""); // a double quote inside a single-quoted string
            } else if c == '\n' {
                out.push_str("\\n");
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                out.push('"');
                last = '"';
            }
            '{' | '[' => {
                stack.push(if c == '{' { '}' } else { ']' });
                out.push(c);
                last = c;
            }
            '}' | ']' => {
                // a trailing comma before the closer
                let trimmed = out.trim_end().len();
                if out[..trimmed].ends_with(',') {
                    out.truncate(trimmed - 1);
                }
                stack.pop();
                out.push(c);
                last = c;
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-' || chars[i] == '.') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
                if (last == '{' || last == ',') && next == Some(':') {
                    out.push_str(&format!("\"{word}\""));
                } else {
                    match word.as_str() {
                        "True" => out.push_str("true"),
                        "False" => out.push_str("false"),
                        "None" | "NULL" | "Null" | "undefined" => out.push_str("null"),
                        _ => out.push_str(&word),
                    }
                }
                last = 'a';
                continue;
            }
            c if c.is_whitespace() => out.push(c),
            c => {
                out.push(c);
                last = c;
            }
        }
        i += 1;
    }
    if quote.is_some() {
        out.push('"');
    }
    let trimmed = out.trim_end().len();
    if out[..trimmed].ends_with(',') {
        out.truncate(trimmed - 1);
    }
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    out
}

/// Parse JSON that a model wrote carelessly. `None` when nothing sensible can be made of it.
pub fn parse_loose(s: &str) -> Option<Value> {
    let t = strip_fences(s);
    if let Ok(v) = serde_json::from_str::<Value>(&t) {
        return Some(v);
    }
    let cand = first_balanced(&t).unwrap_or(&t);
    if let Ok(v) = serde_json::from_str::<Value>(cand) {
        return Some(v);
    }
    // the balanced scan fails on a cut-off object: repair from the first brace to the end
    let from = t.find(['{', '[']).map_or(t.as_str(), |i| &t[i..]);
    let fixed = normalize(from);
    serde_json::from_str::<Value>(&fixed).ok().filter(|v| v.is_object() || v.is_array())
}

// ---------------------------------------------------------------------------------------------------------- names

fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Words models use for tools, by squashed spelling, and the tool they mean.
const ALIASES: &[(&str, &str)] = &[
    ("search", "web_search"),
    ("googlesearch", "web_search"),
    ("internetsearch", "web_search"),
    ("searchweb", "web_search"),
    ("websearch", "web_search"),
    ("research", "web_research"),
    ("webresearcher", "web_research"),
    ("fetch", "web_fetch"),
    ("fetchpage", "web_fetch"),
    ("fetchurl", "web_fetch"),
    ("readpage", "web_fetch"),
    ("browse", "web_fetch"),
    ("openurl", "browser_open"),
    ("openpage", "browser_open"),
    ("openbrowser", "browser_open"),
    ("open", "browser_open"),
    ("getweather", "weather"),
    ("weatherforecast", "weather"),
    ("stockprice", "stock_quote"),
    ("getstockprice", "stock_quote"),
    ("getstockquote", "stock_quote"),
    ("stockquote", "stock_quote"),
    ("quote", "stock_quote"),
    ("screenshot", "screen_capture"),
    ("takescreenshot", "screen_capture"),
    ("capturescreen", "screen_capture"),
    ("screencapture", "screen_capture"),
    ("playradio", "radio_play"),
    ("radio", "radio_play"),
    ("stopradio", "radio_stop"),
    ("playmusic", "media_play"),
    ("addevent", "calendar_add"),
    ("addcalendarevent", "calendar_add"),
    ("setalarm", "alarm_set"),
    ("createstudyplan", "study_plan_create"),
    ("studyplan", "study_plan_create"),
    ("makestudyplan", "study_plan_create"),
    ("lessonplan", "study_plan_create"),
    ("createquiz", "quiz_create"),
    ("makequiz", "quiz_create"),
    ("quizresults", "quiz_results"),
    ("nextquestion", "quiz_next"),
    ("answerquestion", "quiz_answer"),
    ("submitanswer", "quiz_answer"),
    ("openquiz", "quiz_open"),
    ("openlesson", "study_lesson_open"),
    ("studylesson", "study_lesson_open"),
    ("lessonopen", "study_lesson_open"),
    ("readfile", "read_file"),
    ("listdirectory", "list_dir"),
    ("ls", "list_dir"),
    ("currenttime", "now"),
    ("time", "now"),
    ("date", "now"),
    ("moretools", "more_tools"),
];

/// The registry's name for what the model wrote, if it is one of its tools in disguise (case, `.`/`-`/space for `_`, a
/// `functions.` prefix, a few common synonyms).
pub fn canonical_name(raw: &str, names: &[&str]) -> Option<String> {
    let raw = raw.trim();
    if let Some(n) = names.iter().find(|n| **n == raw) {
        return Some((*n).to_string());
    }
    let mut sq = squash(raw);
    for p in ["functions", "function", "tools", "tool", "defaultapi", "api"] {
        if let Some(rest) = sq.strip_prefix(p) {
            if names.iter().any(|n| squash(n) == rest) {
                sq = rest.to_string();
                break;
            }
        }
    }
    if let Some(n) = names.iter().find(|n| squash(n) == sq) {
        return Some((*n).to_string());
    }
    let target = ALIASES.iter().find(|(a, _)| *a == sq)?.1;
    names.iter().find(|n| **n == target).map(|n| (*n).to_string())
}

// ---------------------------------------------------------------------------------------------------------- arguments

fn props(schema: &Value) -> Map<String, Value> {
    schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn required(schema: &Value) -> Vec<String> {
    schema.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn coerce_value(v: Value, ty: &str, items: Option<&Value>) -> Value {
    match (ty, v) {
        ("integer", Value::String(s)) => s.trim().parse::<i64>().map(Value::from).or_else(|_| s.trim().parse::<f64>().map(|f| Value::from(f.round() as i64))).unwrap_or(Value::String(s)),
        ("integer", Value::Number(n)) if n.as_i64().is_none() => n.as_f64().map_or(Value::Number(n.clone()), |f| Value::from(f.round() as i64)),
        ("number", Value::String(s)) => s.trim().parse::<f64>().ok().and_then(|f| serde_json::Number::from_f64(f).map(Value::Number)).unwrap_or(Value::String(s)),
        ("boolean", Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" | "on" => Value::Bool(true),
            "false" | "no" | "0" | "off" => Value::Bool(false),
            _ => Value::String(s),
        },
        ("boolean", Value::Number(n)) => Value::Bool(n.as_f64().is_some_and(|f| f != 0.0)),
        ("string", Value::Number(n)) => Value::String(n.to_string()),
        ("string", Value::Bool(b)) => Value::String(b.to_string()),
        ("array", Value::String(s)) => match parse_loose(&s) {
            Some(a @ Value::Array(_)) => a,
            _ => Value::Array(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(|x| coerce_value(Value::String(x.to_string()), items.and_then(|i| i.get("type")).and_then(Value::as_str).unwrap_or("string"), None)).collect()),
        },
        ("array", v @ (Value::Number(_) | Value::Bool(_))) => Value::Array(vec![v]),
        ("object", Value::String(s)) => parse_loose(&s).filter(Value::is_object).unwrap_or(Value::String(s)),
        (_, v) => v,
    }
}

/// Arguments in the shape the schema asks for: an object, with the wrapper keys models add taken off, argument names
/// matched without regard to case, values converted to the declared type, nulls dropped.
pub fn coerce_args(args: &Value, schema: &Value) -> Value {
    let p = props(schema);
    let mut v = args.clone();
    for _ in 0..3 {
        match v {
            Value::String(ref s) => {
                v = match parse_loose(s) {
                    Some(o @ Value::Object(_)) => o,
                    _ => {
                        // a bare string for a tool with one required text argument
                        let req = required(schema);
                        match req.as_slice() {
                            [only] if p.get(only).and_then(|x| x.get("type")).and_then(Value::as_str) == Some("string") => json!({ only: s.trim() }),
                            _ => json!({}),
                        }
                    }
                };
            }
            Value::Object(ref m) => {
                if let Some(Value::String(raw)) = m.get("_raw_arguments") {
                    v = parse_loose(raw).filter(Value::is_object).unwrap_or_else(|| json!({}));
                    continue;
                }
                // {"arguments": {...}} / {"parameters": ...} when the tool has no such argument itself
                let wrap = ["arguments", "parameters", "args", "input"].iter().find(|k| m.len() == 1 && m.contains_key(**k) && !p.contains_key(**k));
                match wrap {
                    Some(k) => v = m[*k].clone(),
                    None => break,
                }
            }
            Value::Null => {
                v = json!({});
            }
            _ => {
                v = json!({});
            }
        }
    }
    let Value::Object(m) = v else { return json!({}) };
    let mut out = Map::new();
    for (k, val) in m {
        if val.is_null() {
            continue;
        }
        let key = if p.contains_key(&k) { k } else { p.keys().find(|x| squash(x) == squash(&k)).cloned().unwrap_or(k) };
        let val = match p.get(&key) {
            Some(def) => coerce_value(val, def.get("type").and_then(Value::as_str).unwrap_or(""), def.get("items")),
            None => val,
        };
        out.insert(key, val);
    }
    Value::Object(out)
}

/// `web_search(query: string*, limit: integer)`: one line a model can follow (`*` = required).
pub fn schema_hint(spec: &ToolSpec) -> String {
    let req = required(&spec.schema);
    let mut ordered: Vec<(String, Value)> = props(&spec.schema).into_iter().collect();
    ordered.sort_by_key(|(k, _)| !req.contains(k));
    let parts: Vec<String> = ordered
        .iter()
        .map(|(k, d)| {
            let ty = d.get("type").and_then(Value::as_str).unwrap_or("any");
            let en = d.get("enum").and_then(Value::as_array).map(|e| format!(" one of {}", e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("|"))).unwrap_or_default();
            format!("{k}: {ty}{en}{}", if req.contains(k) { "*" } else { "" })
        })
        .collect();
    format!("{}({})", spec.name, parts.join(", "))
}

/// Check coerced arguments; the error says what is missing or wrong and shows the valid form.
pub fn validate(args: &Value, spec: &ToolSpec) -> Result<(), String> {
    let p = props(&spec.schema);
    let missing: Vec<String> = required(&spec.schema).into_iter().filter(|k| args.get(k).is_none_or(|v| v.is_null())).collect();
    let mut wrong: Vec<String> = Vec::new();
    for (k, def) in &p {
        let Some(v) = args.get(k) else { continue };
        if let Some(e) = def.get("enum").and_then(Value::as_array) {
            if !e.contains(v) {
                wrong.push(format!("`{k}` must be one of {}", e.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")));
                continue;
            }
        }
        let ok = match def.get("type").and_then(Value::as_str) {
            Some("string") => v.is_string(),
            Some("integer") => v.is_i64() || v.is_u64(),
            Some("number") => v.is_number(),
            Some("boolean") => v.is_boolean(),
            Some("array") => v.is_array(),
            Some("object") => v.is_object(),
            _ => true,
        };
        if !ok {
            wrong.push(format!("`{k}` must be a {}", def.get("type").and_then(Value::as_str).unwrap_or("value")));
        }
    }
    if missing.is_empty() && wrong.is_empty() {
        return Ok(());
    }
    let mut msg = String::new();
    if !missing.is_empty() {
        msg.push_str(&format!("missing required argument{} {}. ", if missing.len() == 1 { "" } else { "s" }, missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")));
    }
    if !wrong.is_empty() {
        msg.push_str(&format!("{}. ", wrong.join("; ")));
    }
    Err(format!("{}Valid form: {} (* = required). Call it again with the arguments filled in.", msg, schema_hint(spec)))
}

// ---------------------------------------------------------------------------------------------------------- calls

/// One call object in any of the shapes models print.
fn call_from_value(v: &Value, names: &[&str], n: usize) -> Option<ToolCall> {
    let o = v.as_object()?;
    // {"type": "function", "function": {"name": ..., "arguments": ...}}
    if let Some(f) = o.get("function").filter(|f| f.is_object()) {
        return call_from_value(f, names, n);
    }
    let raw_name = ["name", "tool", "tool_name", "function", "action"].iter().find_map(|k| o.get(*k).and_then(Value::as_str))?;
    let name = canonical_name(raw_name, names)?;
    let args = ["arguments", "parameters", "args", "input", "action_input"].iter().find_map(|k| o.get(*k).cloned()).unwrap_or_else(|| {
        // {"name": "weather", "place": "Chennai"}: the rest of the object is the arguments
        let rest: Map<String, Value> = o.iter().filter(|(k, _)| !matches!(k.as_str(), "name" | "tool" | "tool_name" | "function" | "action" | "type" | "id")).map(|(k, v)| (k.clone(), v.clone())).collect();
        Value::Object(rest)
    });
    Some(ToolCall { id: format!("text_{n}"), name, args })
}

fn calls_from_value(v: &Value, names: &[&str], n0: usize) -> Vec<ToolCall> {
    match v {
        Value::Array(a) => a.iter().enumerate().filter_map(|(i, x)| call_from_value(x, names, n0 + i)).collect(),
        other => call_from_value(other, names, n0).into_iter().collect(),
    }
}

/// Tool calls a model wrote into its answer text instead of using native tool calling: fenced ```tool / ```json blocks,
/// `<tool_call>{...}</tool_call>` (Qwen, Hermes), or an answer that is only one JSON call. Returns the calls and the text
/// without them. Anything that is not a call to a known tool stays in the text.
pub fn extract_text_calls(text: &str, names: &[&str]) -> (Vec<ToolCall>, String) {
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut rest = String::new();
    let mut s = text;
    while !s.is_empty() {
        let fence = s.find("```");
        let tag = s.find("<tool_call>");
        let (at, is_tag) = match (fence, tag) {
            (Some(f), Some(t)) if t < f => (t, true),
            (Some(f), _) => (f, false),
            (None, Some(t)) => (t, true),
            (None, None) => break,
        };
        rest.push_str(&s[..at]);
        let after = &s[at..];
        let (body, consumed) = if is_tag {
            let inner = &after["<tool_call>".len()..];
            match inner.find("</tool_call>") {
                Some(e) => (inner[..e].to_string(), "<tool_call>".len() + e + "</tool_call>".len()),
                None => (inner.to_string(), after.len()), // cut off: the rest is the call
            }
        } else {
            let inner = &after[3..];
            let (head, body_start) = inner.split_once('\n').map_or(("", inner), |(h, b)| (h, b));
            let _ = head;
            match body_start.find("```") {
                Some(e) => (body_start[..e].to_string(), 3 + (inner.len() - body_start.len()) + e + 3),
                None => (body_start.to_string(), after.len()),
            }
        };
        let found = parse_loose(&body).map(|v| calls_from_value(&v, names, calls.len())).unwrap_or_default();
        if found.is_empty() {
            rest.push_str(&after[..consumed]); // an ordinary code block: keep it
        } else {
            calls.extend(found);
        }
        s = &after[consumed..];
    }
    rest.push_str(s);
    // pretty-printed JSON objects or arrays in the middle of the prose (no fence, text before and after)
    let rest = strip_inline_calls(&rest, names, &mut calls);
    if calls.is_empty() {
        // the whole answer is one JSON call, with no fence
        let t = rest.trim();
        if t.starts_with('{') || t.starts_with('[') {
            if let Some(v) = parse_loose(t) {
                let found = calls_from_value(&v, names, 0);
                if !found.is_empty() {
                    return (found, String::new());
                }
            }
        }
    }
    let rest = rest.trim().to_string();
    (calls, rest)
}

/// The end (exclusive byte index) of the balanced JSON object or array that starts at `s[0]`, string-aware.
fn balanced_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, c) in b.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if *c == b'\\' {
                esc = true;
            } else if *c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Calls written as bare JSON (one or several, pretty-printed over many lines, with prose around them): they are taken out of the text
/// and added to `calls`. Objects that are not calls to a known tool stay in the text.
fn strip_inline_calls(text: &str, names: &[&str], calls: &mut Vec<ToolCall>) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        let Some(off) = text[i..].find(['{', '[']) else { break };
        let at = i + off;
        out.push_str(&text[i..at]);
        match balanced_end(&text[at..]) {
            Some(len) => {
                let found = parse_loose(&text[at..at + len]).map(|v| calls_from_value(&v, names, calls.len())).unwrap_or_default();
                if found.is_empty() {
                    out.push_str(&text[at..at + len]);
                } else {
                    calls.extend(found);
                }
                i = at + len;
            }
            None => {
                out.push_str(&text[at..at + 1]);
                i = at + 1;
            }
        }
    }
    out.push_str(&text[i.min(text.len())..]);
    out
}

/// Make one call right: the registry's name for the tool, arguments in the schema's shape.
pub fn repair_call(call: &ToolCall, specs: &[ToolSpec]) -> ToolCall {
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    let name = canonical_name(&call.name, &names).unwrap_or_else(|| call.name.clone());
    let args = match specs.iter().find(|s| s.name == name) {
        Some(s) => coerce_args(&call.args, &s.schema),
        None => call.args.clone(),
    };
    ToolCall { id: call.id.clone(), name, args }
}

/// The instructions for models that cannot call tools natively: print a fenced `tool` block, one per call.
pub fn text_protocol_prompt(specs: &[ToolSpec]) -> String {
    let mut s = String::from("TOOL PROTOCOL: you cannot call tools directly here, so to use one print ONLY a fenced block, nothing else in that reply:\n```tool\n{\"name\": \"TOOL_NAME\", \"arguments\": {\"arg\": \"value\"}}\n```\nUse one block per call, then wait: the result comes back in the next message. When you have what you need, answer in plain text without a block.\nTools (* = required argument):\n");
    for spec in specs {
        let first = spec.description.split(['.', '\n']).next().unwrap_or("").trim();
        let first: String = first.chars().take(90).collect();
        s.push_str(&format!("- {} {}\n", schema_hint(spec), first));
    }
    s
}

/// A tool-using history as plain messages (for a model that has no native tool calling): calls become the block the model
/// would have printed, results become user messages.
pub fn flatten_for_text_mode(history: &[crate::chat::Message]) -> Vec<crate::chat::Message> {
    use crate::chat::{Message, Role};
    history
        .iter()
        .map(|m| match m.role {
            Role::Assistant if !m.tool_calls.is_empty() => {
                let blocks: Vec<String> = m.tool_calls.iter().map(|c| format!("```tool\n{}\n```", json!({"name": c.name, "arguments": c.args}))).collect();
                Message::assistant(if m.content.is_empty() { blocks.join("\n") } else { format!("{}\n{}", m.content, blocks.join("\n")) })
            }
            Role::Tool => Message::user(format!("Result of {}:\n{}", m.tool_name.clone().unwrap_or_default(), m.content)),
            _ => m.clone(),
        })
        .collect()
}

/// Whether a provider's error says the model has no native tool calling (Ollama: "... does not support tools").
pub fn is_no_tools_error(e: &str) -> bool {
    let l = e.to_lowercase();
    (l.contains("does not support tools") || l.contains("doesn't support tools") || l.contains("tools are not supported") || l.contains("tool use is not supported") || l.contains("does not support function") || l.contains("tool calling is not supported") || l.contains("not support tool"))
        && !l.contains("401")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, props: Value, req: &[&str]) -> ToolSpec {
        ToolSpec { name: name.into(), description: format!("{name} does a thing. More words."), schema: json!({"type": "object", "properties": props, "required": req}) }
    }

    fn specs() -> Vec<ToolSpec> {
        vec![
            spec("web_search", json!({"query": {"type": "string"}, "limit": {"type": "integer"}}), &["query"]),
            spec("browser_open", json!({"url": {"type": "string"}}), &["url"]),
            spec("weather", json!({"place": {"type": "string"}, "days": {"type": "integer"}, "metric": {"type": "boolean"}}), &[]),
            spec("media_tag", json!({"id": {"type": "string"}, "add": {"type": "array", "items": {"type": "string"}}}), &["id"]),
            spec("radio_play", json!({"query": {"type": "string"}}), &["query"]),
        ]
    }

    fn names(s: &[ToolSpec]) -> Vec<&str> {
        s.iter().map(|x| x.name.as_str()).collect()
    }

    #[test]
    fn loose_json_table() {
        let cases: &[(&str, Value)] = &[
            ("{\"a\": 1}", json!({"a": 1})),
            ("```json\n{\"a\": 1}\n```", json!({"a": 1})),
            ("```\n{\"a\": 1}\n```", json!({"a": 1})),
            ("{'a': 'x y', 'b': [1, 2,],}", json!({"a": "x y", "b": [1, 2]})),
            ("{a: 1, b: \"two\"}", json!({"a": 1, "b": "two"})),
            ("{\"a\": True, \"b\": None, \"c\": False}", json!({"a": true, "b": null, "c": false})),
            ("Sure! Here is the call: {\"a\": 1} hope that helps", json!({"a": 1})),
            ("{\"a\": 1, \"b\": \"cut off", json!({"a": 1, "b": "cut off"})),
            ("{“a”: “curly”}", json!({"a": "curly"})),
            ("{'q': 'say \"hi\"'}", json!({"q": "say \"hi\""})),
        ];
        for (input, want) in cases {
            assert_eq!(parse_loose(input).as_ref(), Some(want), "{input}");
        }
        assert_eq!(parse_loose("no json at all"), None);
    }

    #[test]
    fn tool_name_aliases_resolve_only_to_tools_that_exist() {
        let s = specs();
        let n = names(&s);
        for (raw, want) in [
            ("web_search", Some("web_search")),
            ("Web-Search", Some("web_search")),
            ("web.search", Some("web_search")),
            ("functions.web_search", Some("web_search")),
            ("WebSearch", Some("web_search")),
            ("browser-open", Some("browser_open")),
            ("BROWSER_OPEN", Some("browser_open")),
            ("search", Some("web_search")),
            ("get_weather", Some("weather")),
            ("play_radio", Some("radio_play")),
            ("screenshot", None), // screen_capture is not in this registry: no pretending
            ("delete_everything", None),
        ] {
            assert_eq!(canonical_name(raw, &n).as_deref(), want, "{raw}");
        }
    }

    #[test]
    fn arguments_are_coerced_to_the_schema() {
        let s = specs();
        let ws = &s[0].schema;
        assert_eq!(coerce_args(&json!("{\"query\": \"x\"}"), ws), json!({"query": "x"}));
        assert_eq!(coerce_args(&json!("tamil news"), ws), json!({"query": "tamil news"}), "a bare string for the one required text argument");
        assert_eq!(coerce_args(&json!({"query": "x", "limit": "5"}), ws), json!({"query": "x", "limit": 5}));
        assert_eq!(coerce_args(&json!({"Query": "x", "limit": 2.0}), ws), json!({"query": "x", "limit": 2}), "names match without case");
        assert_eq!(coerce_args(&json!({"arguments": {"query": "x"}}), ws), json!({"query": "x"}), "wrapper keys come off");
        assert_eq!(coerce_args(&json!({"parameters": "{'query': 'y'}"}), ws), json!({"query": "y"}));
        assert_eq!(coerce_args(&json!({"query": "x", "limit": null}), ws), json!({"query": "x"}));
        assert_eq!(coerce_args(&json!({"_raw_arguments": "{'query': 'z',}"}), ws), json!({"query": "z"}), "the broken JSON a provider could not parse");
        let wx = &s[2].schema;
        assert_eq!(coerce_args(&json!({"metric": "yes", "days": "3", "place": 12}), wx), json!({"metric": true, "days": 3, "place": "12"}));
        let mt = &s[3].schema;
        assert_eq!(coerce_args(&json!({"id": "a", "add": "kuthu, melody"}), mt), json!({"id": "a", "add": ["kuthu", "melody"]}));
        assert_eq!(coerce_args(&json!({"id": "a", "add": "[\"x\"]"}), mt), json!({"id": "a", "add": ["x"]}));
    }

    #[test]
    fn validation_errors_tell_the_model_what_to_do() {
        let s = specs();
        assert!(validate(&json!({"query": "x"}), &s[0]).is_ok());
        let e = validate(&json!({}), &s[0]).unwrap_err();
        assert!(e.contains("missing required argument `query`") && e.contains("web_search(query: string*, limit: integer)"), "{e}");
        let e = validate(&json!({"query": "x", "limit": "many"}), &s[0]).unwrap_err();
        assert!(e.contains("`limit` must be a integer") || e.contains("`limit` must be"), "{e}");
        let en = ToolSpec { name: "t".into(), description: "".into(), schema: json!({"properties": {"k": {"type": "string", "enum": ["a", "b"]}}, "required": ["k"]}) };
        assert!(validate(&json!({"k": "c"}), &en).unwrap_err().contains("one of \"a\", \"b\""));
    }

    #[test]
    fn calls_printed_in_the_answer_are_found_in_every_shape() {
        let s = specs();
        let n = names(&s);
        let cases = [
            "```tool\n{\"name\": \"web_search\", \"arguments\": {\"query\": \"nifty\"}}\n```",
            "I will look it up.\n```json\n{\"name\": \"Web.Search\", \"parameters\": {\"query\": \"nifty\"}}\n```",
            "<tool_call>\n{\"name\": \"web_search\", \"arguments\": {\"query\": \"nifty\"}}\n</tool_call>",
            "{\"name\": \"web_search\", \"arguments\": \"{'query': 'nifty'}\"}",
            "{\"type\": \"function\", \"function\": {\"name\": \"web_search\", \"arguments\": {\"query\": \"nifty\",}}}",
            "[{\"tool\": \"web_search\", \"query\": \"nifty\"}]",
            "{\n  \"name\": \"web_search\",\n  \"arguments\": {\n    \"query\": \"nifty\"\n  }\n}",
        ];
        for c in cases {
            let (calls, _) = extract_text_calls(c, &n);
            assert_eq!(calls.len(), 1, "{c}");
            let fixed = repair_call(&calls[0], &s);
            assert_eq!((fixed.name.as_str(), fixed.args), ("web_search", json!({"query": "nifty"})), "{c}");
        }
        let (calls, rest) = extract_text_calls("Checking.\n```tool\n{\"name\": \"weather\", \"arguments\": {\"place\": \"Chennai\"}}\n```\n```tool\n{\"name\": \"radio_play\", \"arguments\": {\"query\": \"tamil\"}}\n```", &n);
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["weather", "radio_play"]);
        assert_eq!(rest, "Checking.");
        // ordinary code blocks and JSON for unknown tools stay in the text
        let (calls, rest) = extract_text_calls("Here:\n```json\n{\"name\": \"rm_rf\", \"arguments\": {}}\n```\n```rust\nfn main() {}\n```", &n);
        assert!(calls.is_empty() && rest.contains("rm_rf") && rest.contains("fn main"));
        let (calls, rest) = extract_text_calls("Use {braces} freely, no call here.", &n);
        assert!(calls.is_empty() && rest == "Use {braces} freely, no call here.");
    }

    #[test]
    fn text_mode_helpers() {
        let s = specs();
        let p = text_protocol_prompt(&s);
        assert!(p.contains("```tool") && p.contains("web_search(query: string*, limit: integer)") && p.contains("browser_open(url: string*)"));
        assert!(is_no_tools_error("HTTP 400: registry.ollama.ai/library/deepseek-r1:1.5b does not support tools"));
        assert!(!is_no_tools_error("HTTP 401: bad key"));
        use crate::chat::Message;
        let c = ToolCall { id: "1".into(), name: "weather".into(), args: json!({"place": "Chennai"}) };
        let flat = flatten_for_text_mode(&[Message::user("hi"), Message::assistant_calls("", vec![c.clone()]), Message::tool_result(&c, "31 C")]);
        assert!(flat[1].tool_calls.is_empty() && flat[1].content.contains("```tool") && flat[1].content.contains("Chennai"));
        assert_eq!(flat[2].role, crate::chat::Role::User);
        assert!(flat[2].content.starts_with("Result of weather:\n31 C"));
    }

    #[test]
    fn pretty_printed_json_calls_are_found_in_every_shape_including_more_tools() {
        let names = ["more_tools", "weather", "run_command"];
        let exact = "{\n  \"name\": \"more_tools\",\n  \"arguments\": {\n    \"group\": \"system\"\n  }\n}";
        let (c, rest) = extract_text_calls(exact, &names);
        assert_eq!((c.len(), c[0].name.as_str(), c[0].args["group"].as_str(), rest.as_str()), (1, "more_tools", Some("system"), ""));
        // with prose before and after, in a json fence, and with the key variants
        let (c, rest) = extract_text_calls(&format!("I need the system tools first.\n{exact}\nThen I will continue."), &names);
        assert_eq!((c.len(), rest.as_str()), (1, "I need the system tools first.\n\nThen I will continue."));
        let (c, rest) = extract_text_calls(&format!("```json\n{exact}\n```"), &names);
        assert_eq!((c.len(), rest.as_str()), (1, ""));
        for variant in [r#"{"tool": "weather", "parameters": {"place": "Chennai"}}"#, r#"{"function": {"name": "weather", "arguments": {"place": "Chennai"}}}"#, r#"{"name": "weather", "args": {"place": "Chennai"}}"#, r#"{"tool_name": "weather", "input": {"place": "Chennai"}}"#] {
            let pretty = serde_json::to_string_pretty(&serde_json::from_str::<Value>(variant).unwrap()).unwrap();
            let (c, rest) = extract_text_calls(&format!("Checking:\n{pretty}\n"), &names);
            assert_eq!((c.len(), c[0].name.as_str(), c[0].args["place"].as_str(), rest.as_str()), (1, "weather", Some("Chennai"), "Checking:"), "{variant}");
        }
        // several calls in an array, an object that is not a call, and braces inside strings
        let (c, _) = extract_text_calls("[{\"name\": \"weather\", \"arguments\": {\"place\": \"A\"}},\n {\"name\": \"more_tools\", \"arguments\": {\"group\": \"web\"}}]", &names);
        assert_eq!(c.len(), 2);
        let (c, rest) = extract_text_calls("Here is the config: {\"port\": 3000, \"note\": \"a } brace\"} done.", &names);
        assert!(c.is_empty() && rest.contains("\"port\": 3000") && rest.contains("a } brace"), "{rest}");
        let (c, _) = extract_text_calls("{\"name\": \"unknown_tool\", \"arguments\": {}}", &names);
        assert!(c.is_empty(), "only known tools");
    }
}
