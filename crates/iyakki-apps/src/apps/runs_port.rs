//! What the RUNS tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`runs.rs`) names no `Services`, no session and no audit log: it asks a [`RunsPort`] for the signed-in person's runs. The real port
//! ([`CxPort`]) is the only place that reads the person's name from the session, calls the core's run store for that person (the store never
//! shows another person's runs) and writes the audit lines.

use iyakki_shared::runs::{Integrity, RunLog, RunMeta};

use super::AppCx;

pub trait RunsPort {
    /// The recorded runs of the signed-in person, newest first.
    fn list(&self) -> Vec<RunMeta>;
    /// One run's record and whether its hash chain is intact.
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)>;
    /// Delete one of the person's runs.
    fn delete(&self, id: &str) -> Result<(), String>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl RunsPort for CxPort<'_> {
    fn list(&self) -> Vec<RunMeta> {
        self.0.runs().list(&self.0.session.user)
    }
    fn load(&self, id: &str) -> Option<(RunLog, Integrity)> {
        self.0.runs().load(&self.0.session.user, id).ok()
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.runs().delete(&self.0.session.user, id)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
