//! What the AUTH tile (one-time codes) asks of the rest of the system: the vault its codes are kept in and the audit log. The tile
//! (`mod.rs`) names no `Services`, no `AppCx` and no session; the real port ([`CxPort`]) is the only place that does. Tests use `AuthApp::with_vault`
//! with a fake vault and clock, or the fake port in `fake_tests.rs`.

use iyakki_shared::authcodes::CodeVault;

use crate::apps::AppCx;

pub trait AuthPort {
    /// The vault the signed-in person's codes are kept in.
    fn vault(&self) -> Box<dyn CodeVault>;
    /// One audit line of the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl AuthPort for CxPort<'_> {
    fn vault(&self) -> Box<dyn CodeVault> {
        self.0.services.code_vault(&self.0.session.user)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
