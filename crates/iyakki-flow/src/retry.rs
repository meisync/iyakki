//! Retry policies: how many attempts, how long each may take, how long to wait between them, and what is worth retrying.
//! Pure maths, no clock and no threads, so every rule is a unit test. See `docs/FLOWS.md` ("Retry, timeout, backoff").

use serde::{Deserialize, Serialize};

pub const MAX_ATTEMPTS: u32 = 10;
pub const MAX_DELAY_MS: u64 = 3_600_000;
/// The longest a `Retry-After` is honoured when the policy sets no `max_ms`.
pub const RETRY_AFTER_CAP_MS: u64 = 300_000;

/// One entry of a per-attempt list: the time attempt *n* may take and the pause before it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptStep {
    pub timeout_ms: u64,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backoff {
    /// The same pause before every retry.
    Fixed { delay_ms: u64 },
    /// `min(max_ms, base_ms * factor^(n-2))` before attempt n (n >= 2), plus or minus `jitter` (0 to 1) of itself.
    Exponential {
        base_ms: u64,
        #[serde(default = "two")]
        factor: f64,
        #[serde(default = "ten_s")]
        max_ms: u64,
        #[serde(default)]
        jitter: f64,
    },
    /// Attempt n uses `steps[n-1]` (the last entry repeats): its timeout and the pause before it.
    List { steps: Vec<AttemptStep> },
}

fn two() -> f64 {
    2.0
}
fn ten_s() -> u64 {
    10_000
}
fn yes() -> bool {
    true
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff::Fixed { delay_ms: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    /// Attempts in all (1 = no retry). With a `List` backoff and 0 here the list's length is used.
    #[serde(default = "one")]
    pub attempts: u32,
    #[serde(default)]
    pub backoff: Backoff,
    /// A fixed per-attempt timeout (the `List` backoff has its own per attempt).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Stop retrying when the next wait would pass this many ms after the first attempt began.
    #[serde(default)]
    pub total_deadline_ms: Option<u64>,
    #[serde(default = "yes")]
    pub honour_retry_after: bool,
}

fn one() -> u32 {
    1
}

impl Default for RetryPolicy {
    fn default() -> RetryPolicy {
        RetryPolicy { attempts: 1, backoff: Backoff::default(), timeout_ms: None, total_deadline_ms: None, honour_retry_after: true }
    }
}

impl RetryPolicy {
    pub fn none() -> RetryPolicy {
        RetryPolicy::default()
    }

    pub fn fixed(attempts: u32, delay_ms: u64) -> RetryPolicy {
        RetryPolicy { attempts, backoff: Backoff::Fixed { delay_ms }, ..Default::default() }
    }

    pub fn exponential(attempts: u32, base_ms: u64, factor: f64, max_ms: u64, jitter: f64) -> RetryPolicy {
        RetryPolicy { attempts, backoff: Backoff::Exponential { base_ms, factor, max_ms, jitter }, ..Default::default() }
    }

    pub fn list(steps: &[(u64, u64)]) -> RetryPolicy {
        RetryPolicy { attempts: 0, backoff: Backoff::List { steps: steps.iter().map(|&(t, d)| AttemptStep { timeout_ms: t, delay_ms: d }).collect() }, ..Default::default() }
    }

    /// Attempts in all, within the hard cap.
    pub fn total_attempts(&self) -> u32 {
        let n = match (&self.backoff, self.attempts) {
            (Backoff::List { steps }, 0) => steps.len() as u32,
            (_, 0) => 1,
            (_, n) => n,
        };
        n.clamp(1, MAX_ATTEMPTS)
    }

    /// The time attempt `n` (1-based) may take: the list's entry, else the policy's, else `fallback`.
    pub fn timeout_for(&self, n: u32, fallback: u64) -> u64 {
        if let Backoff::List { steps } = &self.backoff {
            if let Some(s) = steps.get((n.max(1) as usize - 1).min(steps.len().saturating_sub(1))) {
                if s.timeout_ms > 0 {
                    return s.timeout_ms;
                }
            }
        }
        self.timeout_ms.filter(|t| *t > 0).unwrap_or(fallback)
    }

    /// The pause before attempt `n` (n >= 2 for the first retry); 0 for the first attempt. `retry_after_ms` is what the server asked.
    /// `seed` makes the jitter repeatable (a hash of run, node and attempt).
    pub fn delay_before(&self, n: u32, retry_after_ms: Option<u64>, seed: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        let computed = match &self.backoff {
            Backoff::Fixed { delay_ms } => *delay_ms,
            Backoff::Exponential { base_ms, factor, max_ms, jitter } => {
                let f = if factor.is_finite() && *factor >= 1.0 { *factor } else { 2.0 };
                let raw = (*base_ms as f64) * f.powi((n - 2).min(30) as i32);
                let capped = raw.min(*max_ms as f64).max(0.0);
                let j = jitter.clamp(0.0, 1.0);
                if j == 0.0 {
                    capped as u64
                } else {
                    // a value in [-1, 1] from the seed
                    let u = (mix(seed ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) % 2001) as f64 / 1000.0 - 1.0;
                    (capped * (1.0 + j * u)).max(0.0) as u64
                }
            }
            Backoff::List { steps } => steps.get((n as usize - 1).min(steps.len().saturating_sub(1))).map_or(0, |s| s.delay_ms),
        };
        let d = match retry_after_ms {
            Some(ra) if self.honour_retry_after => ra.min(RETRY_AFTER_CAP_MS),
            _ => computed,
        };
        d.min(MAX_DELAY_MS)
    }

    /// May attempt `n` (1-based) start, given when the first began, now, and the wait that would come before it?
    pub fn within_deadline(&self, first_started_ms: i64, now_ms: i64, wait_ms: u64) -> bool {
        match self.total_deadline_ms {
            Some(limit) => (now_ms - first_started_ms).max(0) as u64 + wait_ms <= limit,
            None => true,
        }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.attempts > MAX_ATTEMPTS {
            v.push(format!("attempts is at most {MAX_ATTEMPTS}"));
        }
        match &self.backoff {
            Backoff::Exponential { factor, jitter, base_ms, max_ms } => {
                if !factor.is_finite() || *factor < 1.0 {
                    v.push("the exponential factor must be at least 1".into());
                }
                if !(0.0..=1.0).contains(jitter) {
                    v.push("jitter is between 0 and 1".into());
                }
                if max_ms < base_ms {
                    v.push("max_ms is smaller than base_ms".into());
                }
            }
            Backoff::List { steps } if steps.is_empty() => v.push("the per-attempt list is empty".into()),
            Backoff::List { steps } if steps.iter().any(|s| s.delay_ms > MAX_DELAY_MS) => v.push("a delay is over an hour".into()),
            Backoff::Fixed { delay_ms } if *delay_ms > MAX_DELAY_MS => v.push("the delay is over an hour".into()),
            _ => {}
        }
        v
    }

    /// One line for the screen: `3 attempts, 700/400 ms timeouts` and so on.
    pub fn describe(&self) -> String {
        let n = self.total_attempts();
        if n == 1 {
            return "no retry".into();
        }
        match &self.backoff {
            Backoff::Fixed { delay_ms } => format!("{n} attempts, {delay_ms} ms apart"),
            Backoff::Exponential { base_ms, factor, max_ms, .. } => format!("{n} attempts, {base_ms} ms x{factor} up to {max_ms} ms"),
            Backoff::List { steps } => format!("{n} attempts, timeouts {} ms", steps.iter().map(|s| s.timeout_ms.to_string()).collect::<Vec<_>>().join("/")),
        }
    }
}

/// SplitMix64.
pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// FNV-1a over parts, for jitter seeds and keys.
pub fn hash_parts(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0xff)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

// ------------------------------------------------------------------------------------------------ error classes

/// What went wrong, in the terms the retry rules use. Kinds fill the fields that apply.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct NodeError {
    pub message: String,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// A gRPC status name such as `UNAVAILABLE`.
    #[serde(default)]
    pub grpc_code: Option<String>,
    #[serde(default)]
    pub timed_out: bool,
    /// Connection refused or reset, DNS failure, TLS handshake dropped.
    #[serde(default)]
    pub connection: bool,
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    /// The program, the CLI or the engine is missing (never retried).
    #[serde(default)]
    pub missing: bool,
    /// A browser navigation timed out or an element was not there yet.
    #[serde(default)]
    pub navigation: bool,
    /// The kind marked this transient (a provider error) or permanent.
    #[serde(default)]
    pub transient: Option<bool>,
    /// Combined output of a command, for pattern matching (kept short).
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub cancelled: bool,
    /// The configuration is wrong; the person or the agent must change it.
    #[serde(default)]
    pub config: bool,
    /// The step needs a command-line tool that is missing or too old: `(cli id, minimum version, version found or empty)`.
    #[serde(default)]
    pub needs_cli: Option<(String, String, String)>,
    /// Only the person can fix this: a permission, consent or trust setting is off (camera not allowed, a browser session not
    /// approved). The agent is never asked to fix it and no fix attempt is spent; the card tells the person what to switch on.
    #[serde(default)]
    pub needs_person: bool,
    /// The permission to ask the person for (`camera`, `camera_unattended`), empty when there is nothing to ask.
    #[serde(default)]
    pub permission: String,
}

impl NodeError {
    pub fn new(message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), ..Default::default() }
    }
    pub fn config(message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), config: true, ..Default::default() }
    }
    /// A refusal that only the person can lift (a consent or trust setting). It is a configuration error that the agent never gets to "fix".
    pub fn consent(message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), config: true, needs_person: true, ..Default::default() }
    }
    /// A step that needs the person to make a choice (no vision model is available, say): like a consent failure, never sent to the
    /// fixing agent, and no permission request is made for it.
    pub fn choice(message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), config: true, needs_person: true, permission: "choice".into(), ..Default::default() }
    }
    /// A consent failure the person can settle by answering a permission request.
    pub fn consent_for(permission: &str, message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), config: true, needs_person: true, permission: permission.into(), ..Default::default() }
    }
    pub fn timeout(message: impl Into<String>) -> NodeError {
        NodeError { message: message.into(), timed_out: true, ..Default::default() }
    }
    pub fn cancelled() -> NodeError {
        NodeError { message: "cancelled".into(), cancelled: true, ..Default::default() }
    }
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrClass {
    Retryable,
    Permanent,
}

/// 408, 425, 429 and the 5xx that mean "try again" (not 501 Not Implemented or 505).
pub fn http_status_retryable(s: u16) -> bool {
    matches!(s, 408 | 425 | 429) || (500..600).contains(&s) && !matches!(s, 501 | 505)
}

pub fn grpc_code_retryable(code: &str) -> bool {
    matches!(code.to_ascii_uppercase().as_str(), "UNAVAILABLE" | "DEADLINE_EXCEEDED" | "RESOURCE_EXHAUSTED" | "ABORTED")
}

/// Parse a `Retry-After` header value: whole seconds, or an HTTP date relative to `now_unix` (seconds). Result in ms.
pub fn parse_retry_after(v: &str, now_unix: i64) -> Option<u64> {
    let v = v.trim();
    if let Ok(s) = v.parse::<u64>() {
        return Some(s.saturating_mul(1000).min(MAX_DELAY_MS));
    }
    let t = parse_http_date(v)?;
    Some(((t - now_unix).max(0) as u64).saturating_mul(1000).min(MAX_DELAY_MS))
}

/// `Sun, 06 Nov 1994 08:49:37 GMT` to unix seconds.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches("GMT").trim();
    let s = s.split_once(',').map_or(s, |x| x.1).trim();
    let mut it = s.split_whitespace();
    let day: u32 = it.next()?.parse().ok()?;
    let mon = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| it.clone().next().is_some_and(|x| x.to_ascii_lowercase().starts_with(m)))? as u32 + 1;
    it.next();
    let year: i64 = it.next()?.parse().ok()?;
    let hms = it.next()?;
    let mut p = hms.split(':');
    let (h, m, sec): (i64, i64, i64) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    Some(crate::sched::days_from_civil(year, mon, day) * 86400 + h * 3600 + m * 60 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_exponential_and_lists() {
        let p = RetryPolicy::fixed(4, 500);
        assert_eq!(p.total_attempts(), 4);
        assert_eq!(p.delay_before(1, None, 0), 0);
        assert_eq!((2..=4).map(|n| p.delay_before(n, None, 0)).collect::<Vec<_>>(), vec![500, 500, 500]);
        let e = RetryPolicy::exponential(5, 100, 2.0, 700, 0.0);
        assert_eq!((2..=5).map(|n| e.delay_before(n, None, 0)).collect::<Vec<_>>(), vec![100, 200, 400, 700]);
        // the example from the request: run 1 may take 700 ms, run 2 400 ms after a 200 ms pause, and so on
        let l = RetryPolicy::list(&[(700, 0), (400, 200), (300, 500)]);
        assert_eq!(l.total_attempts(), 3);
        assert_eq!((1..=3).map(|n| l.timeout_for(n, 9999)).collect::<Vec<_>>(), vec![700, 400, 300]);
        assert_eq!((1..=3).map(|n| l.delay_before(n, None, 0)).collect::<Vec<_>>(), vec![0, 200, 500]);
        let mut l4 = l.clone();
        l4.attempts = 5;
        assert_eq!(l4.total_attempts(), 5);
        assert_eq!(l4.timeout_for(5, 1), 300, "the last entry repeats");
        assert_eq!(l4.delay_before(5, None, 0), 500);
        assert_eq!(RetryPolicy::none().timeout_for(1, 1234), 1234);
        let mut t = RetryPolicy::fixed(2, 0);
        t.timeout_ms = Some(900);
        assert_eq!(t.timeout_for(2, 5), 900);
    }

    #[test]
    fn jitter_stays_inside_its_bounds_and_repeats() {
        let e = RetryPolicy::exponential(6, 1000, 2.0, 60_000, 0.25);
        for seed in 0..500u64 {
            for n in 2..=6u32 {
                let base = (1000.0 * 2f64.powi(n as i32 - 2)).min(60_000.0);
                let d = e.delay_before(n, None, seed) as f64;
                assert!(d >= base * 0.75 - 1.0 && d <= base * 1.25 + 1.0, "n={n} seed={seed} d={d}");
                assert_eq!(e.delay_before(n, None, seed), e.delay_before(n, None, seed));
            }
        }
        let distinct: std::collections::BTreeSet<u64> = (0..50u64).map(|s| e.delay_before(3, None, s)).collect();
        assert!(distinct.len() > 10, "the jitter varies");
    }

    #[test]
    fn retry_after_wins_and_is_capped() {
        let p = RetryPolicy::fixed(3, 100);
        assert_eq!(p.delay_before(2, Some(2500), 0), 2500);
        assert_eq!(p.delay_before(2, Some(10_000_000), 0), RETRY_AFTER_CAP_MS);
        let mut q = p.clone();
        q.honour_retry_after = false;
        assert_eq!(q.delay_before(2, Some(2500), 0), 100);
        assert_eq!(parse_retry_after("3", 0), Some(3000));
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:47 GMT", parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap()), Some(10_000));
        assert_eq!(parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", parse_http_date("Sun, 06 Nov 1994 09:00:00 GMT").unwrap()), Some(0));
        assert_eq!(parse_retry_after("soon", 0), None);
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:10 GMT"), Some(10));
    }

    #[test]
    fn deadline_and_validation() {
        let mut p = RetryPolicy::fixed(5, 1000);
        p.total_deadline_ms = Some(3000);
        assert!(p.within_deadline(0, 1000, 1000));
        assert!(!p.within_deadline(0, 2500, 1000));
        assert!(RetryPolicy::exponential(3, 100, 0.5, 50, 2.0).validate().len() >= 3);
        assert!(RetryPolicy::list(&[]).validate().len() == 1);
        assert!(RetryPolicy::fixed(3, 10).validate().is_empty());
        assert_eq!(RetryPolicy::fixed(50, 1).total_attempts(), MAX_ATTEMPTS);
        assert_eq!(RetryPolicy::list(&[(700, 0), (400, 200)]).describe(), "2 attempts, timeouts 700/400 ms");
    }

    #[test]
    fn status_and_grpc_classes() {
        for s in [408, 425, 429, 500, 502, 503, 504] {
            assert!(http_status_retryable(s), "{s}");
        }
        for s in [200, 301, 400, 401, 403, 404, 410, 422, 501, 505] {
            assert!(!http_status_retryable(s), "{s}");
        }
        for c in ["UNAVAILABLE", "deadline_exceeded", "RESOURCE_EXHAUSTED"] {
            assert!(grpc_code_retryable(c));
        }
        for c in ["INVALID_ARGUMENT", "NOT_FOUND", "PERMISSION_DENIED", "UNAUTHENTICATED", "UNIMPLEMENTED"] {
            assert!(!grpc_code_retryable(c));
        }
    }

    #[test]
    fn policies_round_trip_as_json() {
        let j = r#"{"attempts":3,"backoff":{"kind":"list","steps":[{"timeout_ms":700},{"timeout_ms":400,"delay_ms":200}]}}"#;
        let p: RetryPolicy = serde_json::from_str(j).unwrap();
        assert_eq!(p.timeout_for(2, 0), 400);
        assert_eq!(serde_json::from_str::<RetryPolicy>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
        let d: RetryPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(d.total_attempts(), 1);
    }
}
