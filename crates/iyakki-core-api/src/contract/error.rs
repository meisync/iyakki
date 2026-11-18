//! One error type for every contract call: a stable code, a stable message key a UI can translate, plain words and a hint.

use serde::{Deserialize, Serialize};
use std::fmt;

pub type CallResult<T> = Result<T, CallError>;

/// Stable codes. Never renamed or reused once released; new ones are only added (a client treats an unknown code as `internal`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Denied,
    Unsupported,
    InvalidInput,
    Busy,
    Io,
    Unauthenticated,
    RateLimited,
    /// A person at the machine has to answer first.
    NeedsConsent,
    /// The vault (or another lock) is closed.
    Locked,
    /// The request clashes with what exists (an idempotency key reused for a different request).
    Conflict,
    /// A queued request or consent prompt ran out of time.
    Expired,
    /// The machine is off or not connected (the relay answers this, never the machine).
    Offline,
    VersionMismatch,
    Cancelled,
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 16] = [
        ErrorCode::NotFound,
        ErrorCode::Denied,
        ErrorCode::Unsupported,
        ErrorCode::InvalidInput,
        ErrorCode::Busy,
        ErrorCode::Io,
        ErrorCode::Unauthenticated,
        ErrorCode::RateLimited,
        ErrorCode::NeedsConsent,
        ErrorCode::Locked,
        ErrorCode::Conflict,
        ErrorCode::Expired,
        ErrorCode::Offline,
        ErrorCode::VersionMismatch,
        ErrorCode::Cancelled,
        ErrorCode::Internal,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotFound => "not_found",
            ErrorCode::Denied => "denied",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::InvalidInput => "invalid_input",
            ErrorCode::Busy => "busy",
            ErrorCode::Io => "io",
            ErrorCode::Unauthenticated => "unauthenticated",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::NeedsConsent => "needs_consent",
            ErrorCode::Locked => "locked",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Expired => "expired",
            ErrorCode::Offline => "offline",
            ErrorCode::VersionMismatch => "version_mismatch",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Internal => "internal",
        }
    }
    /// May a client repeat the same request (with the same idempotency key) and expect it to be harmless? Only conditions that
    /// are about timing, never about the request itself.
    pub fn safe_to_retry(self) -> bool {
        matches!(self, ErrorCode::Busy | ErrorCode::RateLimited | ErrorCode::Offline | ErrorCode::Io)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallError {
    pub code: ErrorCode,
    /// Stable key such as `settings.unknown_key`; a UI looks its own translation up by this.
    pub message_key: String,
    /// The same thing in plain English words.
    pub message: String,
    /// What to do next, in plain words (may be empty).
    pub hint: String,
    pub retry_after_ms: Option<u32>,
}

impl CallError {
    pub fn new(code: ErrorCode, message_key: &str, message: &str) -> CallError {
        CallError { code, message_key: message_key.to_string(), message: message.to_string(), hint: String::new(), retry_after_ms: None }
    }
    pub fn hint(mut self, hint: &str) -> CallError {
        self.hint = hint.to_string();
        self
    }
    pub fn retry_after(mut self, ms: u32) -> CallError {
        self.retry_after_ms = Some(ms);
        self
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CallError {}

impl From<crate::error::ApiError> for CallError {
    fn from(e: crate::error::ApiError) -> CallError {
        use crate::error::ApiError as A;
        let (code, text) = match &e {
            A::NotFound(m) => (ErrorCode::NotFound, m),
            A::Denied(m) => (ErrorCode::Denied, m),
            A::Unsupported(m) => (ErrorCode::Unsupported, m),
            A::InvalidInput(m) => (ErrorCode::InvalidInput, m),
            A::Busy(m) => (ErrorCode::Busy, m),
            A::Io(m) => (ErrorCode::Io, m),
        };
        CallError::new(code, &format!("core.{}", e.code()), text)
    }
}

/// A caller-chosen key that makes a change safe to repeat: the same key with the same request gives the same result and does
/// the work once. 8 to 64 characters of `A-Z a-z 0-9 _ -`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(key: &str) -> CallResult<IdempotencyKey> {
        let ok = (8..=64).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if ok {
            Ok(IdempotencyKey(key.to_string()))
        } else {
            Err(CallError::new(ErrorCode::InvalidInput, "core.bad_idempotency_key", "The idempotency key must be 8 to 64 letters, digits, - or _."))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How many calls a caller may make in a window; sent in `hello` so a client can pace itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    pub calls: u32,
    pub per_ms: u32,
}

/// A sliding-window limiter with time passed in (no clock here, so tests are exact).
#[derive(Debug)]
pub struct RateLimiter {
    limit: RateLimit,
    stamps: std::collections::VecDeque<u64>,
}

impl RateLimiter {
    pub fn new(limit: RateLimit) -> RateLimiter {
        RateLimiter { limit, stamps: Default::default() }
    }
    pub fn check(&mut self, now_ms: u64) -> CallResult<()> {
        while self.stamps.front().is_some_and(|t| now_ms.saturating_sub(*t) >= self.limit.per_ms as u64) {
            self.stamps.pop_front();
        }
        if self.stamps.len() as u32 >= self.limit.calls {
            let wait = self.stamps.front().map(|t| (*t + self.limit.per_ms as u64).saturating_sub(now_ms)).unwrap_or(0);
            return Err(CallError::new(ErrorCode::RateLimited, "core.rate_limited", "Too many requests; slow down.").retry_after(wait.min(u32::MAX as u64) as u32));
        }
        self.stamps.push_back(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_golden_list() {
        let golden = [
            "not_found", "denied", "unsupported", "invalid_input", "busy", "io", "unauthenticated", "rate_limited", "needs_consent", "locked",
            "conflict", "expired", "offline", "version_mismatch", "cancelled", "internal",
        ];
        let now: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(now, golden, "codes are never renamed, reordered or removed; add new ones at the end");
        for c in ErrorCode::ALL {
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{}\"", c.as_str()), "wire form equals as_str");
        }
    }

    #[test]
    fn the_old_api_error_codes_map_to_the_same_strings() {
        use crate::error::ApiError as A;
        for (e, code) in [(A::NotFound("x".into()), "not_found"), (A::Denied("x".into()), "denied"), (A::Unsupported("x".into()), "unsupported"), (A::InvalidInput("x".into()), "invalid_input"), (A::Busy("x".into()), "busy"), (A::Io("x".into()), "io")] {
            assert_eq!(CallError::from(e).code.as_str(), code);
        }
    }

    #[test]
    fn only_timing_errors_are_safe_to_retry() {
        let safe: Vec<&str> = ErrorCode::ALL.iter().filter(|c| c.safe_to_retry()).map(|c| c.as_str()).collect();
        assert_eq!(safe, ["busy", "io", "rate_limited", "offline"]);
    }

    #[test]
    fn idempotency_keys_are_checked() {
        assert!(IdempotencyKey::new("run-0001-abc").is_ok());
        assert!(IdempotencyKey::new("short").is_err());
        assert!(IdempotencyKey::new("has space in it").is_err());
        assert!(IdempotencyKey::new(&"x".repeat(65)).is_err());
    }

    #[test]
    fn the_limiter_refuses_with_the_wait_and_recovers() {
        let mut l = RateLimiter::new(RateLimit { calls: 2, per_ms: 1000 });
        assert!(l.check(0).is_ok());
        assert!(l.check(100).is_ok());
        let e = l.check(200).unwrap_err();
        assert_eq!((e.code, e.retry_after_ms), (ErrorCode::RateLimited, Some(800)));
        assert!(l.check(1000).is_ok(), "the first call left the window");
    }
}
