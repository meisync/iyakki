//! `iyakki dev web ADDRESS[:PORT] --fingerprint FP runs|flows|flow-run FLOW|flow-runs FLOW [--follow]`: the program client's commands for
//! the web service's chat runs and flow runs (F-WEBSVC-14). With `--follow` the command listens to the service's event stream (and opens
//! it again when it drops) instead of asking again and again: `flow-run --follow` ends when the run it started ends.

use std::cell::RefCell;
use std::time::Duration;

use serde_json::Value;

use super::cli::Args;
use super::client::{split_host, Home};
use super::webclient::{event_line, ApiClient};

pub const USAGE: &str = "  web ADDRESS[:PORT] --fingerprint FP runs|flows|flow-run FLOW|flow-runs FLOW [--follow] [--seconds N] [--retries N] [--json]   the web service's chat runs and flows; --follow prints their progress as it happens (scope api:chat or api:flows)";

pub(super) fn run(cmd: &str, args: &Args, home: &Home, out: &mut dyn FnMut(&str)) -> Option<Result<(), String>> {
    if cmd != "web" {
        return None;
    }
    Some(web(args, home, out))
}

/// Does this event say something about the run being followed? `flow`/`run` empty means every run of its kind.
pub fn event_matches(v: &Value, kind: &str, flow: &str, run: &str) -> bool {
    let t = v["type"].as_str().unwrap_or("");
    if t != kind {
        return false;
    }
    let d = &v["data"];
    kind != "flowrun" || ((flow.is_empty() || d["flow"] == flow) && (run.is_empty() || d["run"] == run))
}

/// A flow run that has reached its end.
pub fn is_final(v: &Value) -> bool {
    matches!(v["data"]["status"].as_str(), Some("failed" | "succeeded" | "cancelled"))
}

fn web(args: &Args, home: &Home, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let usage = "usage: iyakki dev web ADDRESS[:PORT] --fingerprint FP runs|flows|flow-run FLOW|flow-runs FLOW [--follow] [--seconds N] [--retries N] [--json]";
    let (host, port) = split_host(args.pos.get(1).ok_or(usage)?, 8443)?;
    let sub = args.pos.get(2).map(String::as_str).ok_or(usage)?;
    let flow = args.pos.get(3).map(String::as_str).unwrap_or("");
    let fp = args.get("--fingerprint").ok_or("the service's certificate fingerprint is needed (`iyakki serve` prints it): --fingerprint FP")?;
    let seconds: u64 = args.get("--seconds").map(|v| v.parse().map_err(|_| "--seconds needs a number".to_string())).transpose()?.unwrap_or(300).clamp(1, 86_400);
    let retries: u32 = args.get("--retries").map(|v| v.parse().map_err(|_| "--retries needs a number".to_string())).transpose()?.unwrap_or(5).min(100);
    let (json, follow) = (args.has("--json"), args.has("--follow"));
    let api = ApiClient::new(home, &host, port, fp)?;
    api.sign_in()?;
    let sink = RefCell::new(out);
    let say = |s: &str| (sink.borrow_mut())(s);
    let show = |v: &Value| say(&if json { v.to_string() } else { event_line(v) });
    let ask = |method: &str, path: &str, body: Option<&Value>| -> Result<Value, String> {
        let (st, v) = api.call(method, path, &[], body)?;
        if st >= 400 {
            return Err(v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("the service answered {st}")));
        }
        Ok(v)
    };
    let need_flow = || if flow.is_empty() || flow.contains('/') || flow.chars().any(char::is_whitespace) { Err(format!("name the flow by its id: {usage}")) } else { Ok(()) };
    // (kind of event to follow, flow, run to stop at)
    let (kind, watch_flow, watch_run): (&str, String, String) = match sub {
        "runs" => {
            let v = ask("GET", "/api/v1/chat/runs", None)?;
            for r in v["runs"].as_array().into_iter().flatten() {
                show(r);
            }
            ("run", String::new(), String::new())
        }
        "flows" => {
            let v = ask("GET", "/api/v1/flows", None)?;
            for f in v["flows"].as_array().into_iter().flatten() {
                show(f);
            }
            ("flowrun", String::new(), String::new())
        }
        "flow-runs" => {
            need_flow()?;
            let v = ask("GET", &format!("/api/v1/flows/{flow}/runs"), None)?;
            for r in v["runs"].as_array().into_iter().flatten() {
                show(r);
            }
            ("flowrun", flow.to_string(), String::new())
        }
        "flow-run" => {
            need_flow()?;
            let v = ask("POST", &format!("/api/v1/flows/{flow}/run"), Some(&serde_json::json!({})))?;
            show(&v);
            ("flowrun", flow.to_string(), v["run"].as_str().unwrap_or("").to_string())
        }
        _ => return Err(usage.into()),
    };
    if !follow {
        return Ok(());
    }
    let started_run = !watch_run.is_empty();
    let n = api.follow_reconnecting(
        Duration::from_secs(seconds),
        100_000,
        retries,
        Duration::from_secs(30),
        &mut |v| {
            // a run that ended before the stream opened (or while it was down) is read once the stream says it is ready
            if started_run && v["type"] == "ready" {
                if let Ok(view) = ask("GET", &format!("/api/v1/flows/{watch_flow}/runs/{watch_run}"), None) {
                    if matches!(view["status"].as_str(), Some("failed" | "succeeded" | "cancelled")) {
                        show(&serde_json::json!({"type": "flowrun", "data": {"flow": watch_flow, "run": watch_run, "status": view["status"]}}));
                        return false;
                    }
                }
                return true;
            }
            if !event_matches(v, kind, &watch_flow, &watch_run) {
                return true;
            }
            show(v);
            // a run this command started ends the command when it ends
            !(started_run && is_final(v))
        },
        &mut |n, why| say(&if json { serde_json::json!({"type": "reconnecting", "attempt": n, "why": why}).to_string() } else { format!("reconnecting (attempt {n}): {why}") }),
    )?;
    if !json {
        say(&format!("{n} event(s) seen."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_picked_by_kind_flow_and_run() {
        let a = json!({"type": "flowrun", "data": {"flow": "f1", "run": "r1", "status": "running"}});
        assert!(event_matches(&a, "flowrun", "", ""));
        assert!(event_matches(&a, "flowrun", "f1", "r1"));
        assert!(!event_matches(&a, "flowrun", "f2", ""));
        assert!(!event_matches(&a, "flowrun", "f1", "r2"));
        assert!(!event_matches(&a, "run", "", ""), "a flow run is not a chat run");
        assert!(!event_matches(&json!({"type": "ready"}), "run", "", ""));
        assert!(event_matches(&json!({"type": "run", "data": {"id": "c1", "status": "running"}}), "run", "", ""));
        assert!(!event_matches(&json!({"type": "changed", "data": {"area": "x"}}), "run", "", ""));
        assert!(!event_matches(&json!({"type": "approval", "data": {"run": "data"}}), "run", "", ""));
    }

    #[test]
    fn only_the_three_end_states_are_final() {
        assert!(["succeeded", "failed", "cancelled"].iter().all(|w| is_final(&json!({"type": "flowrun", "data": {"status": w}}))));
        assert!(!is_final(&json!({"data": {"status": "running"}})) && !is_final(&json!({"data": {"status": "waiting"}})) && !is_final(&json!({})));
    }
}
