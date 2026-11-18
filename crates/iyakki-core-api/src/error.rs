//! One error type for every call. The codes are stable strings: they are what a remote UI sees and what tests compare.

use serde::{Deserialize, Serialize};

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum ApiError {
    /// The thing asked for does not exist (no such device, file, key).
    #[error("not found: {0}")]
    NotFound(String),
    /// The caller's identity or scope does not allow it.
    #[error("denied: {0}")]
    Denied(String),
    /// This machine or build cannot do it (no battery, no camera, not a PC).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The request itself is wrong (bad value, too long).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Something else holds the resource right now; try again.
    #[error("busy: {0}")]
    Busy(String),
    /// The system call or read failed.
    #[error("io: {0}")]
    Io(String),
}

impl ApiError {
    /// The stable machine-readable code (never changes once released).
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::NotFound(_) => "not_found",
            ApiError::Denied(_) => "denied",
            ApiError::Unsupported(_) => "unsupported",
            ApiError::InvalidInput(_) => "invalid_input",
            ApiError::Busy(_) => "busy",
            ApiError::Io(_) => "io",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_match_the_wire_form() {
        let all = [
            (ApiError::NotFound("x".into()), "not_found"),
            (ApiError::Denied("x".into()), "denied"),
            (ApiError::Unsupported("x".into()), "unsupported"),
            (ApiError::InvalidInput("x".into()), "invalid_input"),
            (ApiError::Busy("x".into()), "busy"),
            (ApiError::Io("x".into()), "io"),
        ];
        for (e, code) in all {
            assert_eq!(e.code(), code);
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, format!("{{\"code\":\"{code}\",\"message\":\"x\"}}"));
            assert_eq!(serde_json::from_str::<ApiError>(&json).unwrap(), e);
        }
    }

    #[test]
    fn the_display_text_names_the_kind() {
        assert_eq!(ApiError::Busy("card".into()).to_string(), "busy: card");
    }
}
