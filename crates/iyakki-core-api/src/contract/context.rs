//! Who is calling, over what, with which scopes, and the one check every call makes first.

use super::error::{CallError, CallResult, ErrorCode};
use crate::identity::Identity;
use serde::{Deserialize, Serialize};

/// The three transports behind one typed API. Only [`Transport::InProcess`] is the machine's own screen and keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A tile or the shell in the same process: a person at the machine.
    InProcess,
    /// The TLS developer-mode channel (a paired computer, signed requests).
    DevMode,
    /// The cloud relay; the machine dialled out, no port is open.
    Relay,
}

impl Transport {
    pub fn is_local(self) -> bool {
        self == Transport::InProcess
    }
}

/// A scope id such as `settings:write`. Remote callers need the scope named; local ones are limited by the person's role.
/// Existing developer-mode scopes (`debug`, `config`, `files`, ...) are valid ids too, so one grant list serves both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope(String);

impl Scope {
    pub const SETTINGS_READ: &'static str = "settings:read";
    pub const SETTINGS_WRITE: &'static str = "settings:write";
    pub const VAULT_STATUS: &'static str = "vault:status";
    pub const VAULT_LOCK: &'static str = "vault:lock";
    pub const VAULT_USE: &'static str = "vault:use";
    pub const FLOWS_READ: &'static str = "flows:read";
    pub const FLOWS_RUN: &'static str = "flows:run";
    pub const FLOWS_APPROVE: &'static str = "flows:approve";
    pub const CONSENT_READ: &'static str = "consent:read";
    /// Not a grantable scope: the marker of calls that only the local screen can make.
    pub const LOCAL: &'static str = "local";

    /// A scope id is lower-case letters, digits, `:`, `_` and `-`, 1 to 40 characters.
    pub fn new(id: &str) -> Option<Scope> {
        let ok = !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b':' | b'_' | b'-'));
        ok.then(|| Scope(id.to_string()))
    }
    pub fn id(&self) -> &str {
        &self.0
    }
}

/// How dangerous a call is, which decides who may make it and what is logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reads state; no side effect.
    Read,
    /// Changes state the caller's scope covers.
    Change,
    /// Starts something a person at the machine must confirm (a consent request is raised; the answer is local).
    AskPerson,
    /// Only the machine's own screen may make it (unlock, answer a consent prompt, pair a client).
    LocalOnly,
}

impl Risk {
    pub fn id(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Change => "change",
            Risk::AskPerson => "ask_person",
            Risk::LocalOnly => "local_only",
        }
    }
}

/// Everything a call knows about its caller. The transport fills it in; a tile cannot make a remote caller look local.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallContext {
    pub identity: Identity,
    pub transport: Transport,
}

impl CallContext {
    pub fn local(user: &str, admin: bool) -> CallContext {
        CallContext { identity: Identity::local(user, admin), transport: Transport::InProcess }
    }
    pub fn remote(user: &str, transport: Transport, scopes: &[&str]) -> CallContext {
        CallContext { identity: Identity::remote(user, scopes), transport }
    }
    pub fn is_local(&self) -> bool {
        self.transport.is_local()
    }
    /// Local: the signed-in person (role decides elsewhere). Remote: the scope must be named; no scopes means nothing allowed.
    pub fn allows(&self, scope: &str) -> bool {
        if self.is_local() {
            !self.identity.user.is_empty()
        } else {
            self.identity.scopes.iter().any(|s| s == scope)
        }
    }
    /// The audit line for a call: who, over what, which call, the outcome. Never contains values or secrets.
    pub fn audit_line(&self, call: &str, risk: Risk, outcome: &str) -> String {
        let via = match self.transport {
            Transport::InProcess => "local",
            Transport::DevMode => "devmode",
            Transport::Relay => "relay",
        };
        format!("{} via {via}: {call} [{}] {outcome}", self.identity.user, risk.id())
    }
}

/// The first check of every call. `LocalOnly` refuses any remote caller; every other risk needs the scope when remote.
pub fn guard(ctx: &CallContext, scope: &str, risk: Risk) -> CallResult<()> {
    if risk == Risk::LocalOnly && !ctx.is_local() {
        return Err(CallError::new(ErrorCode::Denied, "core.local_only", "This can only be done at the machine itself.").hint("Ask the person at the machine."));
    }
    if !ctx.allows(scope) {
        return Err(CallError::new(ErrorCode::Denied, "core.scope_missing", &format!("This computer is not allowed to do that ({scope}).")).hint("An administrator can grant it on the machine."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_caller_with_no_scopes_is_allowed_nothing_unlike_a_local_one() {
        let remote = CallContext::remote("laptop", Transport::Relay, &[]);
        assert!(!remote.allows(Scope::SETTINGS_READ));
        assert!(CallContext::local("amy", false).allows(Scope::SETTINGS_READ));
    }

    #[test]
    fn local_only_refuses_every_remote_transport_even_with_the_scope() {
        for t in [Transport::DevMode, Transport::Relay] {
            let c = CallContext::remote("laptop", t, &[Scope::LOCAL, Scope::VAULT_USE]);
            assert_eq!(guard(&c, Scope::LOCAL, Risk::LocalOnly).unwrap_err().code, ErrorCode::Denied);
        }
        assert!(guard(&CallContext::local("amy", true), Scope::LOCAL, Risk::LocalOnly).is_ok());
    }

    #[test]
    fn scope_ids_are_checked() {
        assert!(Scope::new("settings:write").is_some());
        assert!(Scope::new("Settings").is_none());
        assert!(Scope::new("").is_none());
        assert!(Scope::new("a b").is_none());
    }

    #[test]
    fn the_audit_line_names_who_over_what_and_the_call() {
        let c = CallContext::remote("laptop", Transport::DevMode, &[]);
        assert_eq!(c.audit_line("settings.set", Risk::Change, "ok"), "laptop via devmode: settings.set [change] ok");
    }
}
