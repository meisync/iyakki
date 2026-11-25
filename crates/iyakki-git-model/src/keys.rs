//! What the git keys and the connections ask of the system: a line in the audit log and the person's vault. The keys themselves stay in
//! `iyakki_shared::gitkeys`, which re-exports these two.

use std::sync::Arc;

/// The type a secret read from the vault comes in (wiped when dropped).
pub use zeroize::Zeroizing;

/// A line in the audit log: the action and the detail (never a value).
pub type AuditFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// What the person's vault offers; the real one is `ServicesStore` (in the agent), tests use `MemStore`.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<zeroize::Zeroizing<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    fn names(&self) -> Result<Vec<String>, String>;
}

/// What an address will use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// Nothing: a public repository over HTTPS, or an SSH address with no key chosen (git will say it cannot log in).
    None,
    Key { id: String, from_workspace: bool },
    Token { host: String },
}

impl Auth {
    /// The word written in project.json and the audit.
    pub fn label(&self) -> String {
        match self {
            Auth::None => String::new(),
            Auth::Key { id, .. } => id.clone(),
            Auth::Token { .. } => "token".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Person,
    Assistant,
}
