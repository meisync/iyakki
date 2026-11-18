//! What a headless web service (`iyakki serve --profile ...`) needs from the machine's account system and audit log, as traits.
//! The service layer in `iyakki-agent` names only these; the supervisor wires the real accounts (which live in the shell crate
//! today) and `iyakki-core-fake` supplies in-memory ones for tests.

use crate::error::ApiResult;
use iyakki_sec::policy::Policy;

/// A signed-in person as the service sees them: name, role and what the role allows.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user: String,
    /// `admin`, `operator`, `user` or `guest`.
    pub role: String,
    pub admin: bool,
    pub policy: Policy,
}

/// The machine's accounts. Implementations must verify a password in the same time for a wrong name and a wrong password,
/// and throttle repeated failures per account (the shell's `UserStore` does both).
pub trait Authenticator: Send + Sync {
    /// Check a name and password. One error for both "no such person" and "wrong password"; `Busy` when throttled.
    fn login(&self, name: &str, password: &str) -> ApiResult<Principal>;
    /// The person as they are now (role and policy may have changed); `None` when the account no longer exists, which ends
    /// every token of that person.
    fn lookup(&self, user: &str) -> Option<Principal>;
}

/// Where audit lines go. Callers never pass secrets (passwords, tokens, prompts).
pub trait AuditSink: Send + Sync {
    fn record(&self, user: &str, event: &str, detail: &str);
}
