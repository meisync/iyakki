//! The QA run report: per node status and timing, assertion results, a gallery of the screenshots, DOM snippets and console errors,
//! built from a finished (or running) [`Run`]. Exported as JSON, and as ONE self-contained HTML file (the pictures embedded).
//!
//! Nothing here reads a secret: outputs and logs in a run are already redacted by the engine. Screenshots are pictures of the page
//! and can show anything that was on the screen, including something typed in the clear; the report says so.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Shot {
    pub node: String,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct NodeLine {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub state: String,
    pub duration_ms: i64,
    pub attempts: usize,
    pub error: String,
    /// `{check, passed, message, soft}` for a check or a baseline comparison.
    pub assertion: Option<Value>,
    pub screenshots: Vec<String>,
    /// URL, title and the start of the text or HTML a DOM node read.
    pub dom: Option<Value>,
    pub console_errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Summary {
    pub nodes: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub checks: usize,
    pub checks_failed: usize,
    pub soft_failed: usize,
    pub screenshots: usize,
    pub console_errors: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct QaReport {
    pub flow_id: String,
    pub flow_name: String,
    pub run_id: String,
    pub status: String,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub duration_ms: i64,
    pub passed: bool,
    pub summary: Summary,
    pub nodes: Vec<NodeLine>,
    pub gallery: Vec<Shot>,
    pub note: String,
}

pub const SECRETS_NOTE: &str = "Typed secrets and password fields are removed from the text in this report. A screenshot is a picture of the screen: it can show anything that was visible when it was taken, including text a person or the flow typed in clear.";

fn file_name(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}

fn snippet(v: &Value, n: usize) -> String {
    let t = v.as_str().unwrap_or("");
    t.chars().take(n).collect()
}

/// Build the report. Nodes are in the order they started (then by id).
pub fn build(run: &Run) -> QaReport {
    let mut lines: Vec<(i64, usize, NodeLine)> = Vec::new();
    let mut gallery: Vec<Shot> = Vec::new();
    let mut sum = Summary::default();
    for (id, nr) in &run.nodes {
        let node = run.flow.node(id);
        let kind = node.map(|n| n.kind.clone()).unwrap_or_default();
        let title = node.map(|n| n.title().to_string()).unwrap_or_else(|| id.clone());
        let browser = kind.starts_with("br_") || kind == "browser";
        let mut line = NodeLine {
            id: id.clone(),
            title,
            kind: kind.clone(),
            state: format!("{:?}", nr.state).to_ascii_lowercase(),
            duration_ms: if nr.ended_ms > nr.started_ms && nr.started_ms > 0 { nr.ended_ms - nr.started_ms } else { 0 },
            attempts: nr.attempts.len(),
            error: nr.error.clone(),
            ..Default::default()
        };
        sum.nodes += 1;
        match nr.state {
            NodeState::Succeeded => sum.succeeded += 1,
            NodeState::Failed => sum.failed += 1,
            NodeState::Skipped => sum.skipped += 1,
            _ => {}
        }
        if browser {
            let o = &nr.output;
            if matches!(kind.as_str(), "br_assert" | "br_compare") {
                let passed = o.get("passed").and_then(Value::as_bool).unwrap_or(nr.state == NodeState::Succeeded);
                let soft = o.get("soft").and_then(Value::as_bool).unwrap_or(false);
                let message = o.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| nr.error.clone());
                line.assertion = Some(json!({"check": o.get("check").cloned().unwrap_or(json!(kind)), "passed": passed && nr.state == NodeState::Succeeded, "message": message, "soft": soft}));
                sum.checks += 1;
                if !passed || nr.state == NodeState::Failed {
                    sum.checks_failed += 1;
                    if soft && nr.state == NodeState::Succeeded {
                        sum.soft_failed += 1;
                    }
                }
            }
            if kind == "br_dom" {
                line.dom = Some(json!({"url": o.get("url").cloned().unwrap_or(Value::Null), "title": o.get("title").cloned().unwrap_or(Value::Null), "snippet": snippet(o.get("text").or_else(|| o.get("html")).or_else(|| o.get("outer_html")).unwrap_or(&Value::Null), 1500), "truncated": o.get("truncated").cloned().unwrap_or(json!(false)), "bytes": o.get("bytes").cloned().unwrap_or(Value::Null)}));
            }
            if let Some(c) = o.get("console").and_then(Value::as_array) {
                for m in c {
                    let level = m.get("level").and_then(Value::as_str).unwrap_or("");
                    if matches!(level, "error" | "exception" | "unhandledrejection") {
                        line.console_errors.push(format!("{level}: {}", m.get("text").and_then(Value::as_str).unwrap_or("")));
                    }
                }
            }
            for p in &nr.artifacts {
                if p.ends_with(".png") {
                    line.screenshots.push(p.clone());
                    gallery.push(Shot { node: id.clone(), name: file_name(p), path: p.clone() });
                }
            }
        }
        sum.console_errors += line.console_errors.len();
        let at = run.flow.nodes.iter().position(|n| n.id == *id).unwrap_or(usize::MAX);
        lines.push((nr.started_ms, at, line));
    }
    // in the order they started; steps that started in the same millisecond keep the flow's order
    lines.sort_by(|a, b| (a.0 == 0, a.0, a.1).cmp(&(b.0 == 0, b.0, b.1)));
    sum.screenshots = gallery.len();
    let passed = run.status == RunStatus::Succeeded && sum.checks_failed == sum.soft_failed;
    QaReport {
        flow_id: run.flow_id.clone(),
        flow_name: run.flow.name.clone(),
        run_id: run.id.clone(),
        status: run.status.label().to_string(),
        started_ms: run.started_ms,
        ended_ms: run.ended_ms,
        duration_ms: if run.ended_ms > run.started_ms { run.ended_ms - run.started_ms } else { 0 },
        passed,
        summary: sum,
        nodes: lines.into_iter().map(|l| l.2).collect(),
        gallery,
        note: SECRETS_NOTE.to_string(),
    }
}

pub fn to_json(r: &QaReport) -> String {
    serde_json::to_string_pretty(r).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------ HTML

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn time_text(ms: i64) -> String {
    if ms >= 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

/// One self-contained HTML page. `load` reads a screenshot file (the path as stored in the run); a missing file is shown as such.
pub fn to_html(r: &QaReport, load: &dyn Fn(&str) -> Option<Vec<u8>>) -> String {
    let mut h = String::new();
    h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    h.push_str(&format!("<title>QA report: {}</title>", esc(&r.flow_name)));
    h.push_str("<style>body{font:15px/1.5 system-ui,sans-serif;margin:0;background:#f6f7f9;color:#1b1f24}main{max-width:1000px;margin:0 auto;padding:20px}h1{font-size:22px;margin:0 0 4px}h2{font-size:17px;margin:28px 0 8px}.pill{display:inline-block;padding:2px 10px;border-radius:99px;font-weight:600;font-size:13px}.ok{background:#d6f2dc;color:#0b5a1d}.bad{background:#fbd9d9;color:#8a1111}.mid{background:#fdf0cc;color:#7a5400}.mut{background:#e6e8ec;color:#444}table{border-collapse:collapse;width:100%;background:#fff;border-radius:8px;overflow:hidden}td,th{padding:7px 10px;text-align:left;border-bottom:1px solid #e6e8ec;vertical-align:top;font-size:14px}th{background:#eef0f3}.gal{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:12px}.gal figure{margin:0;background:#fff;border-radius:8px;padding:8px}.gal img{width:100%;height:auto;border:1px solid #ddd;border-radius:4px}figcaption{font-size:12px;color:#555;margin-top:4px}pre{background:#fff;padding:10px;border-radius:8px;overflow:auto;font-size:12px;white-space:pre-wrap;word-break:break-word}.note{color:#555;font-size:13px}@media(prefers-color-scheme:dark){body{background:#14171a;color:#e6e8ec}table,.gal figure,pre{background:#1d2126}th{background:#262b31}td,th{border-color:#2f353c}.note,figcaption{color:#aab}}</style></head><body><main>");
    let (cls, word) = if r.passed { ("ok", "PASSED") } else if r.status == "failed" || r.summary.failed > 0 || r.summary.checks_failed > 0 { ("bad", "FAILED") } else { ("mid", "NOT FINISHED") };
    h.push_str(&format!("<h1>{}</h1><p><span class=\"pill {cls}\">{word}</span> <span class=\"pill mut\">run {}</span> <span class=\"pill mut\">{}</span> <span class=\"pill mut\">{}</span></p>", esc(&r.flow_name), esc(&r.run_id), esc(&r.status), time_text(r.duration_ms)));
    let s = &r.summary;
    h.push_str(&format!("<p>{} steps: {} done, {} failed, {} skipped. {} checks, {} failed ({} soft). {} screenshots. {} console errors.</p>", s.nodes, s.succeeded, s.failed, s.skipped, s.checks, s.checks_failed, s.soft_failed, s.screenshots, s.console_errors));
    h.push_str("<h2>Steps</h2><table><tr><th>Step</th><th>Kind</th><th>Result</th><th>Time</th><th>Detail</th></tr>");
    for n in &r.nodes {
        let c = match n.state.as_str() {
            "succeeded" => "ok",
            "failed" => "bad",
            "skipped" | "cancelled" => "mut",
            _ => "mid",
        };
        let mut detail = String::new();
        if let Some(a) = &n.assertion {
            let passed = a["passed"].as_bool().unwrap_or(false);
            detail.push_str(&format!("<b>{}</b> {}{}", if passed { "check passed" } else { "check failed" }, esc(a["message"].as_str().unwrap_or("")), if a["soft"].as_bool() == Some(true) { " (soft)" } else { "" }));
        }
        if !n.error.is_empty() {
            detail.push_str(&format!("<div>{}</div>", esc(&n.error)));
        }
        if let Some(d) = &n.dom {
            detail.push_str(&format!("<div>{} &middot; {}</div><pre>{}</pre>", esc(d["url"].as_str().unwrap_or("")), esc(d["title"].as_str().unwrap_or("")), esc(d["snippet"].as_str().unwrap_or(""))));
        }
        for e in &n.console_errors {
            detail.push_str(&format!("<div class=\"pill bad\">{}</div> ", esc(e)));
        }
        h.push_str(&format!("<tr><td>{}<br><span class=\"note\">{}</span></td><td>{}</td><td><span class=\"pill {c}\">{}</span></td><td>{}</td><td>{detail}</td></tr>", esc(&n.title), esc(&n.id), esc(&n.kind), esc(&n.state), time_text(n.duration_ms)));
    }
    h.push_str("</table>");
    h.push_str("<h2>Screenshots</h2>");
    if r.gallery.is_empty() {
        h.push_str("<p class=\"note\">No screenshots in this run.</p>");
    } else {
        h.push_str("<div class=\"gal\">");
        for s in &r.gallery {
            match load(&s.path) {
                Some(bytes) => h.push_str(&format!("<figure><img alt=\"{}\" src=\"data:image/png;base64,{}\"><figcaption>{} &middot; {}</figcaption></figure>", esc(&s.name), base64(&bytes), esc(&s.node), esc(&s.name))),
                None => h.push_str(&format!("<figure><figcaption>{} &middot; {} (the file is no longer there)</figcaption></figure>", esc(&s.node), esc(&s.name))),
            }
        }
        h.push_str("</div>");
    }
    h.push_str(&format!("<p class=\"note\">{}</p></main></body></html>", esc(&r.note)));
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn html_is_escaped() {
        assert_eq!(esc("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
    }
}
