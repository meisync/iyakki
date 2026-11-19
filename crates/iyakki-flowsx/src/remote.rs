//! The remote runner (F-FLOW-16): run one of your flows on ANOTHER IYAKKI and bring back how it went.
//!
//! This is a client of the other machine's own web service (`POST /api/v1/flows/{id}/run`, then `GET .../runs/{run}`), so everything
//! the other machine decides stays decided there: its sign-in, its permission to run flows, its tenant gate, its approvals. A step of
//! the remote run that needs a person is left waiting for THAT person (this side never answers it, and says so). The sign-in is a
//! token the caller holds as a vault secret; it is sent as a bearer header to the address given and nowhere else, and is removed from
//! every message this file produces.
//!
//! Not here: running a single node or a command on the other machine (the other machine has no route that runs a command for a
//! stranger, on purpose), a list of saved remote machines, and a chat tool.

use iyakki_proxy::Upstream;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Another IYAKKI: its address and the token that signs this caller in there.
pub struct RemoteTarget {
    base: String,
    token: Zeroizing<String>,
}

impl RemoteTarget {
    /// `https://` addresses only, except a loopback `http://` (a machine reached through a tunnel). No user name or password in the
    /// address, no query, no fragment.
    pub fn new(base: &str, token: Zeroizing<String>) -> Result<RemoteTarget, String> {
        let b = base.trim().trim_end_matches('/');
        let (scheme, rest) = b.split_once("://").ok_or("the address must start with https://")?;
        if rest.is_empty() || rest.contains(['@', '?', '#', ' ']) || rest.contains('/') {
            return Err("the address is a host and port only, like https://other.local:8443 (no user, path or query)".into());
        }
        let host = rest.rsplit_once(':').map(|x| x.0).unwrap_or(rest).trim_start_matches('[').trim_end_matches(']');
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
        match scheme {
            "https" => {}
            "http" if loopback => {}
            _ => return Err("the address must start with https:// (http:// only for this machine itself, through a tunnel)".into()),
        }
        if token.trim().len() < 8 {
            return Err("the token is missing or too short".into());
        }
        Ok(RemoteTarget { base: b.to_string(), token })
    }

    fn scrub(&self, s: &str) -> String {
        s.replace(self.token.as_str(), "[token]")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteOutcome {
    pub run: String,
    /// The other machine's word: succeeded, failed, cancelled, running, needs you ...
    pub status: String,
    /// The run ended (succeeded, failed or cancelled).
    pub ended: bool,
    /// What a waiting step asks (empty when nothing waits): to be answered on the other machine, by its person.
    pub waiting: String,
    pub error: String,
    /// (step, state)
    pub steps: Vec<(String, String)>,
}

pub struct RunOptions {
    /// How long to follow the run before giving up and returning it as it is (the run itself goes on over there).
    pub wait: Duration,
    pub poll: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions { wait: Duration::from_secs(300), poll: Duration::from_secs(2) }
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn call(up: &dyn Upstream, t: &RemoteTarget, method: &str, path: &str, body: &[u8]) -> Result<Value, String> {
    let headers = vec![("Authorization".to_string(), format!("Bearer {}", t.token.as_str())), ("Content-Type".to_string(), "application/json".to_string()), ("Accept".to_string(), "application/json".to_string())];
    let r = up.request(method, &format!("{}{path}", t.base), &headers, body).map_err(|e| format!("could not reach the other IYAKKI: {}", t.scrub(&e)))?;
    let text = String::from_utf8_lossy(&r.body[..r.body.len().min(256 * 1024)]).into_owned();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match r.status {
        200..=299 => Ok(v),
        401 => Err("the other IYAKKI did not accept the sign-in (the token is wrong or has expired)".into()),
        403 => Err(format!("the other IYAKKI says this sign-in may not do that: {}", t.scrub(v["error"]["message"].as_str().unwrap_or("not allowed")))),
        404 => Err("the other IYAKKI has no such flow or run".into()),
        429 => Err("the other IYAKKI is busy; try again in a minute".into()),
        s => Err(format!("the other IYAKKI answered {s}: {}", t.scrub(v["error"]["message"].as_str().unwrap_or("no details")))),
    }
}

fn outcome(run: &str, v: &Value) -> RemoteOutcome {
    let status = v["status"].as_str().unwrap_or("unknown").to_string();
    RemoteOutcome {
        run: run.to_string(),
        ended: matches!(status.as_str(), "succeeded" | "failed" | "cancelled"),
        status,
        waiting: v["blocked"].as_str().unwrap_or("").chars().take(300).collect(),
        error: v["error"].as_str().unwrap_or("").chars().take(300).collect(),
        steps: v["nodes"].as_array().map(|a| a.iter().map(|n| (n["node"].as_str().unwrap_or("").to_string(), n["state"].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default(),
    }
}

/// Start `flow_id` over there (its published version) with `inputs` and follow it until it ends, a step waits for a person over
/// there, `wait` is up, or `cancel` says stop. The sleeping between looks is `sleep` (a test passes one that does not wait).
pub fn run_flow(up: &dyn Upstream, t: &RemoteTarget, flow_id: &str, inputs: &Value, opts: &RunOptions, cancel: &dyn Fn() -> bool, sleep: &dyn Fn(Duration)) -> Result<RemoteOutcome, String> {
    if !valid_id(flow_id) {
        return Err("a flow id is letters, digits, - and _".into());
    }
    if !(inputs.is_null() || inputs.is_object()) {
        return Err("the inputs are an object".into());
    }
    let body = if inputs.is_null() { json!({}) } else { json!({"inputs": inputs}) };
    let started = call(up, t, "POST", &format!("/api/v1/flows/{flow_id}/run"), body.to_string().as_bytes())?;
    let run = started["run"].as_str().filter(|r| valid_id(r)).ok_or("the other IYAKKI did not name the run it started")?.to_string();
    let begin = Instant::now();
    loop {
        let v = call(up, t, "GET", &format!("/api/v1/flows/{flow_id}/runs/{run}"), b"")?;
        let last = outcome(&run, &v);
        // a waiting step is for the person over there: stop following, never answer it from here
        if last.ended || !last.waiting.is_empty() || cancel() || begin.elapsed() >= opts.wait {
            return Ok(last);
        }
        sleep(opts.poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;
    use std::sync::Mutex;

    struct Script {
        seen: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
        replies: Mutex<Vec<(u16, String)>>,
    }

    impl Script {
        fn new(replies: &[(u16, &str)]) -> Script {
            Script { seen: Mutex::new(vec![]), replies: Mutex::new(replies.iter().rev().map(|(s, b)| (*s, b.to_string())).collect()) }
        }
    }

    impl Upstream for Script {
        fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            self.seen.lock().unwrap().push((method.into(), url.into(), headers.to_vec(), String::from_utf8_lossy(body).into_owned()));
            match self.replies.lock().unwrap().pop() {
                Some((status, b)) if status == 0 => Err(format!("connection refused {b}")),
                Some((status, b)) => Ok(UpstreamResponse { status, body: b.into_bytes() }),
                None => Err("no more replies".into()),
            }
        }
    }

    fn target() -> RemoteTarget {
        RemoteTarget::new("https://other.local:8443/", Zeroizing::new("tok-secret-123".into())).unwrap()
    }

    fn quick() -> RunOptions {
        RunOptions { wait: Duration::from_secs(60), poll: Duration::from_millis(1) }
    }

    #[test]
    fn it_starts_the_flow_and_follows_the_run_to_its_end() {
        let up = Script::new(&[(200, r#"{"run":"r7","flow":"f1"}"#), (200, r#"{"status":"running","nodes":[]}"#), (200, r#"{"status":"succeeded","nodes":[{"node":"a","state":"succeeded"},{"node":"b","state":"succeeded"}]}"#)]);
        let slept = Mutex::new(0);
        let o = run_flow(&up, &target(), "f1", &json!({"x": 1}), &quick(), &|| false, &|_| *slept.lock().unwrap() += 1).unwrap();
        assert_eq!((o.run.as_str(), o.status.as_str(), o.ended), ("r7", "succeeded", true));
        assert_eq!(o.steps, vec![("a".to_string(), "succeeded".to_string()), ("b".to_string(), "succeeded".to_string())]);
        assert_eq!(*slept.lock().unwrap(), 1);
        let seen = up.seen.lock().unwrap();
        assert_eq!(seen[0].1, "https://other.local:8443/api/v1/flows/f1/run");
        assert!(seen[0].3.contains("\"inputs\":{\"x\":1}"));
        assert!(seen[0].2.iter().any(|h| h.0 == "Authorization" && h.1 == "Bearer tok-secret-123"));
        assert_eq!(seen[1].1, "https://other.local:8443/api/v1/flows/f1/runs/r7");
    }

    #[test]
    fn a_step_that_waits_for_a_person_over_there_stops_the_following_and_is_not_answered_from_here() {
        let up = Script::new(&[(200, r#"{"run":"r1"}"#), (200, r#"{"status":"needs you","blocked":"asks: delete the old files?","nodes":[{"node":"a","state":"waiting"}]}"#)]);
        let o = run_flow(&up, &target(), "f1", &Value::Null, &quick(), &|| false, &|_| panic!("must not wait")).unwrap();
        assert!(!o.ended && o.waiting.contains("delete the old files"), "{o:?}");
        let seen = up.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "no approval call was made");
        assert!(seen.iter().all(|s| !s.1.contains("approvals")));
    }

    #[test]
    fn the_wait_budget_and_a_cancel_return_the_run_as_it_is() {
        let running = (200, r#"{"status":"running","nodes":[]}"#);
        let up = Script::new(&[(200, r#"{"run":"r1"}"#), running, running, running]);
        let n = Mutex::new(0);
        let o = run_flow(&up, &target(), "f1", &Value::Null, &quick(), &|| { *n.lock().unwrap() += 1; *n.lock().unwrap() >= 2 }, &|_| {}).unwrap();
        assert_eq!((o.status.as_str(), o.ended), ("running", false));
        let up = Script::new(&[(200, r#"{"run":"r1"}"#), running]);
        let o = run_flow(&up, &target(), "f1", &Value::Null, &RunOptions { wait: Duration::ZERO, poll: Duration::from_millis(1) }, &|| false, &|_| panic!()).unwrap();
        assert!(!o.ended);
    }

    #[test]
    fn refusals_are_said_in_plain_words_and_the_token_never_appears() {
        for (status, want) in [(401, "did not accept the sign-in"), (403, "may not do that"), (404, "no such flow"), (429, "busy"), (500, "answered 500")] {
            let up = Script::new(&[(status, r#"{"error":{"message":"nope tok-secret-123"}}"#)]);
            let e = run_flow(&up, &target(), "f1", &Value::Null, &quick(), &|| false, &|_| {}).unwrap_err();
            assert!(e.contains(want), "{status}: {e}");
            assert!(!e.contains("tok-secret-123"), "{e}");
        }
        let up = Script::new(&[(0, "to tok-secret-123")]);
        let e = run_flow(&up, &target(), "f1", &Value::Null, &quick(), &|| false, &|_| {}).unwrap_err();
        assert!(e.contains("could not reach") && !e.contains("tok-secret-123"), "{e}");
    }

    #[test]
    fn addresses_ids_and_inputs_are_checked_before_anything_is_sent() {
        let tok = || Zeroizing::new("tok-secret-123".to_string());
        for bad in ["http://other.local", "https://u:p@other.local", "https://other.local/x", "https://other.local?a=1", "other.local", "https://"] {
            assert!(RemoteTarget::new(bad, tok()).is_err(), "{bad}");
        }
        assert!(RemoteTarget::new("http://127.0.0.1:8443", tok()).is_ok());
        assert!(RemoteTarget::new("https://other.local", Zeroizing::new("x".into())).is_err());
        let up = Script::new(&[]);
        assert!(run_flow(&up, &target(), "../x", &Value::Null, &quick(), &|| false, &|_| {}).is_err());
        assert!(run_flow(&up, &target(), "f1", &json!([1]), &quick(), &|| false, &|_| {}).is_err());
        assert!(up.seen.lock().unwrap().is_empty());
    }
}
