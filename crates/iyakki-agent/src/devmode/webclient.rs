//! A small client for a program of a paired computer (F-WEBSVC-06): it signs in to the web service with the computer's developer key
//! (no password) and then calls the routes its `api:*` scopes allow. The certificate of the service is pinned by its fingerprint.
//!
//! ```ignore
//! let home = Home::at(&client::default_home());
//! let api = ApiClient::new(&home, "192.168.1.20", 8443, "AB:CD:...")?;
//! let who = api.sign_in()?;                         // {"token", "user", "computer", "scopes": [...]}
//! let (status, flows) = api.call("GET", "/api/v1/flows", &[], None)?;
//! ```
//! The fingerprint is the one the service logs when it starts (and `iyakki serve` prints); the key is the one `iyakki dev pair` made.

use std::sync::Mutex;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use super::auth::sign_headers;
use super::client::{https, target, websocket, Home};

/// Marks an error that a new attempt may cure (the connection, not a refusal).
const RETRYABLE: &str = "connection: ";

enum Ended {
    /// The limit or the event count was reached.
    Done,
    /// The service closed the stream before that.
    Dropped,
}

pub struct ApiClient {
    host: String,
    port: u16,
    fingerprint: String,
    key: SigningKey,
    token: Mutex<Option<String>>,
}

impl ApiClient {
    /// A client for the service at `host:port` whose certificate has this fingerprint, using the key of `home`.
    pub fn new(home: &Home, host: &str, port: u16, fingerprint: &str) -> Result<ApiClient, String> {
        Ok(ApiClient::with_key(host, port, fingerprint, home.key()?))
    }

    pub fn with_key(host: &str, port: u16, fingerprint: &str, key: SigningKey) -> ApiClient {
        ApiClient { host: host.into(), port, fingerprint: fingerprint.into(), key, token: Mutex::new(None) }
    }

    /// Sign in with the developer key. On success the token is kept for [`ApiClient::call`]; the answer says who, which computer and
    /// which scopes. A computer that holds no `api:*` scope is refused (an administrator grants them on the device).
    pub fn sign_in(&self) -> Result<Value, String> {
        let headers = sign_headers(&self.key, "POST", "/api/v1/login", &[], &[], &self.fingerprint, crate::clock::now_unix())?;
        let r = https(&self.host, self.port, Some(&self.fingerprint), "POST", "/api/v1/login", &headers, &[], Duration::from_secs(15))?;
        let v = r.json();
        if r.status != 200 {
            return Err(v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("the service answered {}", r.status)));
        }
        let token = v["token"].as_str().ok_or("the service sent no token")?.to_string();
        *self.token.lock().map_err(|_| "busy".to_string())? = Some(token);
        Ok(v)
    }

    /// One call with the token (sign in first). `Err` only when the service cannot be reached or there is no token; a refusal such as
    /// 403 (the scopes do not allow the route) comes back as the status and the body.
    pub fn call(&self, method: &str, path: &str, query: &[(String, String)], body: Option<&Value>) -> Result<(u16, Value), String> {
        let token = self.token.lock().map_err(|_| "busy".to_string())?.clone().ok_or("sign in first")?;
        let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        if !bytes.is_empty() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let r = https(&self.host, self.port, Some(&self.fingerprint), method, &target(path, query), &headers, &bytes, Duration::from_secs(30))?;
        Ok((r.status, r.json()))
    }

    /// Follow the service's event stream (`/api/v1/events`, a WebSocket) instead of polling: signs in when needed, sends the token as the
    /// first message and hands every event (`ready`, `changed`, `notification`, `count`, `approval`, run progress, flow runs) to
    /// `on_event` as it arrives. Stops after `max_events` events or when `limit` has passed without the stream ending, and returns how
    /// many events were handed over. Heartbeat pings are answered and not counted; a `pong` or an `error` ends the loop with the
    /// service's own words.
    pub fn follow(&self, limit: Duration, max_events: usize, on_event: &mut dyn FnMut(&Value)) -> Result<usize, String> {
        self.follow_once(limit, max_events, &mut |v| {
            on_event(v);
            true
        })
        .map(|(n, _)| n).map_err(|e| e.strip_prefix(RETRYABLE).unwrap_or(&e).to_string())
    }

    /// Like [`ApiClient::follow`], but when the stream drops (the service restarted, the network blinked) it signs in again and opens a
    /// new stream, up to `retries` times, within the same overall `limit` and `max_events`. `on_retry(n, why)` is told before each
    /// new attempt; the wait before it grows (one second, two, four, at most `max_pause`). A refusal in the service's own words (an
    /// `error` event, a failed sign-in) is not retried: it comes back at once. `on_event` answers whether to go on (false ends the
    /// follow, for example when the run being watched has ended).
    pub fn follow_reconnecting(
        &self,
        limit: Duration,
        max_events: usize,
        retries: u32,
        max_pause: Duration,
        on_event: &mut dyn FnMut(&Value) -> bool,
        on_retry: &mut dyn FnMut(u32, &str),
    ) -> Result<usize, String> {
        let end = std::time::Instant::now() + limit;
        let mut total = 0usize;
        let mut attempt = 0u32;
        loop {
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || total >= max_events {
                return Ok(total);
            }
            let why = match self.follow_once(left, max_events - total, on_event) {
                Ok((n, Ended::Done)) => return Ok(total + n),
                Ok((n, Ended::Dropped)) => {
                    total += n;
                    "the stream was closed".to_string()
                }
                Err(e) => match e.strip_prefix(RETRYABLE) {
                    Some(why) => why.to_string(),
                    None => return Err(e),
                },
            };
            attempt += 1;
            if attempt > retries {
                return Err(format!("the stream dropped and {retries} new attempts did not hold: {why}"));
            }
            on_retry(attempt, &why);
            let pause = Duration::from_secs(1u64 << (attempt - 1).min(5)).min(max_pause);
            std::thread::sleep(pause.min(end.saturating_duration_since(std::time::Instant::now())));
            // the token may have gone with the old connection (or the service): sign in again
            if let Ok(mut t) = self.token.lock() {
                *t = None;
            }
        }
    }

    fn follow_once(&self, limit: Duration, max_events: usize, on_event: &mut dyn FnMut(&Value) -> bool) -> Result<(usize, Ended), String> {
        if self.token.lock().map_err(|_| "busy".to_string())?.is_none() {
            self.sign_in().map_err(|e| if e.contains("reach") || e.contains("connect") { format!("{RETRYABLE}{e}") } else { e })?;
        }
        let token = self.token.lock().map_err(|_| "busy".to_string())?.clone().ok_or("sign in first")?;
        let mut ws = websocket(&self.host, self.port, Some(&self.fingerprint), "/api/v1/events", &[], Duration::from_secs(15)).map_err(|e| format!("{RETRYABLE}{e}"))?;
        ws.send_text(&serde_json::json!({"type": "auth", "token": token}).to_string()).map_err(|e| format!("{RETRYABLE}{e}"))?;
        let end = std::time::Instant::now() + limit;
        let mut seen = 0usize;
        while seen < max_events {
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            let asked = left.min(Duration::from_secs(5));
            let started = std::time::Instant::now();
            match ws.recv(asked) {
                Err(e) => return Err(format!("{RETRYABLE}{e}")),
                Ok(Some(iyakki_link::wsproto::Message::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).map_err(|_| "the service sent an event that is not JSON".to_string())?;
                    if v["type"] == "error" {
                        ws.send_close();
                        return Err(v["message"].as_str().unwrap_or("the service refused the stream").to_string());
                    }
                    if v["type"] == "pong" {
                        continue;
                    }
                    seen += 1;
                    if !on_event(&v) {
                        break;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    // nothing within the wait is a quiet stream; an answer well before the wait ran out is a closed one
                    if started.elapsed() + Duration::from_millis(200) < asked {
                        return Ok((seen, Ended::Dropped));
                    }
                }
            }
        }
        ws.send_close();
        Ok((seen, Ended::Done))
    }

    /// End the token at the service.
    pub fn sign_out(&self) -> Result<(), String> {
        let _ = self.call("POST", "/api/v1/logout", &[], None)?;
        *self.token.lock().map_err(|_| "busy".to_string())? = None;
        Ok(())
    }
}

/// One line for an event, for a terminal: the type and the few fields that say what happened (never a token).
pub fn event_line(v: &Value) -> String {
    let kind = v["type"].as_str().unwrap_or("?");
    let mut parts: Vec<String> = Vec::new();
    if let Some(o) = v.as_object() {
        for (k, val) in o {
            if k == "type" || k == "token" {
                continue;
            }
            // the notification bus nests the facts under `data`: show them as if they were on the event
            if let (Some(d), "data") = (val.as_object(), k.as_str()) {
                for (dk, dv) in d.iter().filter(|(dk, _)| dk.as_str() != "token") {
                    match dv {
                        Value::String(s) => parts.push(format!("{dk}={}", s.chars().take(80).collect::<String>())),
                        Value::Number(_) | Value::Bool(_) => parts.push(format!("{dk}={dv}")),
                        _ => {}
                    }
                }
                continue;
            }
            let t = match val {
                Value::String(s) => s.chars().take(80).collect::<String>(),
                Value::Number(_) | Value::Bool(_) => val.to_string(),
                _ => continue,
            };
            parts.push(format!("{k}={t}"));
        }
    }
    if parts.is_empty() { kind.to_string() } else { format!("{kind} {}", parts.join(" ")) }
}
