//! Who is signed in, for the local domain listener's `members` routes (`cloudlisten::MemberGate`).
//!
//! The listener is a plain proxy and cannot know who a person is; the IYAKKI web service does (its session tokens). A web service that
//! starts registers a [`SessionLookup`] here; [`RegistryGate`] asks every live one whether it knows the token a request carried. Nothing
//! is stored here: a lookup that has gone away (the service stopped) is dropped on the next question, and with none registered every
//! request to a `members` route is refused (fail closed).

use std::sync::{Arc, Mutex, OnceLock};

use crate::cloudlisten::{Member, MemberGate};

/// A place that issues session tokens.
pub trait SessionLookup: Send + Sync {
    /// The person a token belongs to (and touch its idle clock, as a request does); `None`: unknown, ended, or the service is gone.
    fn user_of(&self, token: &str) -> Option<String>;
    /// The tenant the session was switched to, if any.
    fn tenant_of(&self, token: &str) -> Option<String>;
    /// False once the service behind this lookup has stopped (it is then dropped).
    fn alive(&self) -> bool {
        true
    }
}

fn registry() -> &'static Mutex<Vec<Arc<dyn SessionLookup>>> {
    static R: OnceLock<Mutex<Vec<Arc<dyn SessionLookup>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// Add a place that issues tokens. Called by the web service when it starts.
pub fn register(l: Arc<dyn SessionLookup>) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).push(l);
}

/// The gate the device's own listener uses: any registered web service may vouch for a token.
pub struct RegistryGate;

impl MemberGate for RegistryGate {
    fn check(&self, session: Option<&str>, tenant: Option<&str>) -> Member {
        let Some(token) = session else { return Member::No };
        let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|l| l.alive());
        for l in g.iter() {
            if let Some(user) = l.user_of(token) {
                // a tenant route is for the people who switched to that tenant
                if tenant.is_none() || l.tenant_of(token).as_deref() == tenant {
                    return Member::Yes(user);
                }
                return Member::No;
            }
        }
        Member::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Fake {
        up: AtomicBool,
    }
    impl SessionLookup for Fake {
        fn user_of(&self, t: &str) -> Option<String> {
            (t == "tok-ann" || t == "tok-bob").then(|| t.trim_start_matches("tok-").to_string())
        }
        fn tenant_of(&self, t: &str) -> Option<String> {
            (t == "tok-bob").then(|| "blue".to_string())
        }
        fn alive(&self) -> bool {
            self.up.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn a_registered_service_vouches_for_its_tokens_and_a_stopped_one_is_dropped() {
        // the registry is process-wide, so this one test owns the whole story
        assert_eq!(RegistryGate.check(Some("tok-ann"), None), Member::No, "nobody registered: fail closed");
        let f = Arc::new(Fake { up: AtomicBool::new(true) });
        register(f.clone());
        assert_eq!(RegistryGate.check(None, None), Member::No);
        assert_eq!(RegistryGate.check(Some("nope"), None), Member::No);
        assert_eq!(RegistryGate.check(Some("tok-ann"), None), Member::Yes("ann".into()));
        assert_eq!(RegistryGate.check(Some("tok-ann"), Some("blue")), Member::No, "a tenant route needs a session switched to that tenant");
        assert_eq!(RegistryGate.check(Some("tok-bob"), Some("blue")), Member::Yes("bob".into()));
        assert_eq!(RegistryGate.check(Some("tok-bob"), Some("red")), Member::No);
        f.up.store(false, Ordering::SeqCst);
        assert_eq!(RegistryGate.check(Some("tok-ann"), None), Member::No, "a stopped service no longer vouches");
    }
}
