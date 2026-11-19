//! What the SKILLS tile asks of the rest of the system (the fourth tile moved onto the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`skills.rs`) holds no `Services`, no `AppCx` and reads no file: it asks a `SkillsPort`. The real port ([`CxPort`]) is the only place that
//! knows the skill store, the package store, the account's policy (who may keep skills of their own and who may manage the shared ones), the read scope
//! of the person for a file they name, and the audit log.

use iyakki_shared::pkg::{Installed, PkgError};
use iyakki_shared::pkg::{Review};
use iyakki_shared::skills::{Scope, Skill, SkillError};

use super::AppCx;

pub trait SkillsPort {
    fn user(&self) -> String;
    /// May this account keep skills of its own?
    fn can_own(&self) -> bool;
    /// May this account add and remove the shared skills and trust a publisher?
    fn can_shared(&self) -> bool;
    /// The skills this account sees: its own first, then the shared ones, each by name.
    fn list(&self) -> Vec<Skill>;
    fn add(&self, scope: Scope, markdown: &str) -> Result<Skill, SkillError>;
    /// Read a package text and say what installing it would do.
    fn review(&self, text: &str, scope: &Scope) -> Result<Review, PkgError>;
    fn install(&self, review: &Review) -> Result<Skill, PkgError>;
    fn remove(&self, scope: &Scope, name: &str) -> Result<(), PkgError>;
    fn trust_key(&self, key: &str, name: &str) -> Result<(), PkgError>;
    /// The trusted publishers as `(key, label)`.
    fn trusted_keys(&self) -> Vec<(String, String)>;
    /// The package a skill came from, when it came from one.
    fn installed(&self, scope: &Scope, name: &str) -> Option<(String, Installed)>;
    /// The text of a file the person named: only inside what the account may read, a text file of at most 64 KiB.
    fn read_text_file(&self, path: &str) -> Result<String, String>;
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl SkillsPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn can_own(&self) -> bool {
        self.0.session.policy.skills
    }
    fn can_shared(&self) -> bool {
        self.0.session.policy.manage_skills
    }
    fn list(&self) -> Vec<Skill> {
        let mut v = self.0.skills().list(&self.0.session.user);
        // mine first, then the shared ones, each by name
        v.sort_by(|a, b| (matches!(a.scope, Scope::Shared), &a.name).cmp(&(matches!(b.scope, Scope::Shared), &b.name)));
        v
    }
    fn add(&self, scope: Scope, markdown: &str) -> Result<Skill, SkillError> {
        self.0.skills().add(scope, markdown, self.can_shared(), self.can_own())
    }
    fn review(&self, text: &str, scope: &Scope) -> Result<Review, PkgError> {
        self.0.packages().review(text, scope)
    }
    fn install(&self, review: &Review) -> Result<Skill, PkgError> {
        self.0.packages().install(review, true, self.can_shared(), self.can_own())
    }
    fn remove(&self, scope: &Scope, name: &str) -> Result<(), PkgError> {
        self.0.packages().remove(scope, name, self.can_shared(), self.can_own())
    }
    fn trust_key(&self, key: &str, name: &str) -> Result<(), PkgError> {
        self.0.packages().trust_key(key, name)
    }
    fn trusted_keys(&self) -> Vec<(String, String)> {
        self.0.packages().trusted_keys()
    }
    fn installed(&self, scope: &Scope, name: &str) -> Option<(String, Installed)> {
        self.0.packages().list(scope).into_iter().find(|(n, _)| n == name)
    }
    fn read_text_file(&self, path: &str) -> Result<String, String> {
        let p = std::path::PathBuf::from(path);
        crate::file_scope().check_read(&self.0.session.policy, &self.0.session.user, &p).and_then(|_| match std::fs::metadata(&p) {
            Ok(m) if m.is_file() && m.len() <= iyakki_shared::pkg::MAX_PACKAGE_BYTES as u64 => std::fs::read_to_string(&p).map_err(|e| e.to_string()),
            Ok(_) => Err("not a text file under 64 KiB".to_string()),
            Err(e) => Err(e.to_string()),
        })
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
