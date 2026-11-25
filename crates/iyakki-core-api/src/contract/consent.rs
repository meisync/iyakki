//! Consent: questions only the local screen can answer. A remote client may list the open questions (to show "waiting for a person
//! at the machine") but the type system gives it no way to answer one: [`Consents::answer`] needs a [`LocalSeat`], and a seat can
//! only be made from a local call context.

use super::context::CallContext;
use super::error::{CallError, CallResult, ErrorCode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentKind {
    Camera,
    Microphone,
    /// A tile asking for a permission.
    Permission,
    Licence,
    VaultUnlock,
    /// A new remote client asking to be paired.
    Pairing,
    Other,
    /// A kind a newer release added; it is shown like `Other` and, because nobody knows what it is, only the machine's own screen may answer it.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRequest {
    pub id: String,
    pub kind: ConsentKind,
    /// Who or what is asking (a tile id, a computer name).
    pub subject: String,
    pub text_key: String,
    pub text: String,
    pub created_ms: u64,
    pub expires_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentAnswer {
    Allow,
    Deny,
}

/// Proof that the caller is the machine's own screen. Cannot be built from a remote context and is never serialised.
#[derive(Debug)]
pub struct LocalSeat {
    user: String,
}

impl LocalSeat {
    pub fn claim(ctx: &CallContext) -> CallResult<LocalSeat> {
        if ctx.is_local() && !ctx.identity.user.is_empty() {
            Ok(LocalSeat { user: ctx.identity.user.clone() })
        } else {
            Err(CallError::new(ErrorCode::Denied, "consent.local_only", "Only a person at the machine can answer this.").hint("Ask the person at the machine to answer on its screen."))
        }
    }
    pub fn user(&self) -> &str {
        &self.user
    }
}

/// Scope for `pending` is `consent:read` (risk read). `answer` is risk `local_only` and has no scope.
pub trait Consents: Send + Sync {
    fn pending(&self, ctx: &CallContext) -> CallResult<Vec<ConsentRequest>>;
    fn answer(&self, seat: &LocalSeat, id: &str, answer: ConsentAnswer) -> CallResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::context::{CallContext, Transport};
    use crate::contract::error::ErrorCode;

    #[test]
    fn a_person_at_the_machine_gets_a_seat_that_knows_who_they_are() {
        let seat = LocalSeat::claim(&CallContext::local("amy", false)).unwrap();
        assert_eq!(seat.user(), "amy");
    }

    #[test]
    fn an_administrator_at_the_machine_gets_a_seat_too() {
        let seat = LocalSeat::claim(&CallContext::local("root", true)).unwrap();
        assert_eq!(seat.user(), "root");
    }

    #[test]
    fn a_local_call_with_no_signed_in_person_gets_no_seat_and_is_told_to_ask_the_person_at_the_machine() {
        let err = LocalSeat::claim(&CallContext::local("", false)).unwrap_err();
        assert!(matches!(err.code, ErrorCode::Denied));
        assert_eq!(err.message_key, "consent.local_only");
        assert_eq!(err.hint, "Ask the person at the machine to answer on its screen.");
    }

    #[test]
    fn every_remote_transport_is_refused_even_when_it_names_a_signed_in_person_and_the_scope() {
        for t in [Transport::DevMode, Transport::Relay, Transport::LocalSocket, Transport::Unknown] {
            let ctx = CallContext::remote("amy", t, &["consent:read", "local"]);
            assert!(LocalSeat::claim(&ctx).is_err(), "{t:?} must not get a seat");
        }
    }
}
