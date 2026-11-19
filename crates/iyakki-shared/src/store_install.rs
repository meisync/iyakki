//! Who is asking the store and what they agreed to (data only). The install itself stays in `iyakki_agent::store::install`.

use crate::skills::Scope;

/// Who is asking, and what their role allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub user: String,
    /// The account may use the STORE tile at all.
    pub allowed: bool,
    /// Install for themselves (policy `skills`).
    pub can_own: bool,
    /// Install for everyone (policy `manage_skills`).
    pub can_shared: bool,
    /// Administrators: servers, unsigned packages, catalogs and keys (policy `manage_settings`).
    pub admin: bool,
}

impl Actor {
    pub fn from_policy(user: &str, p: &iyakki_sec::policy::Policy) -> Actor {
        Actor { user: user.into(), allowed: p.apps.contains(&iyakki_sec::policy::App::Store), can_own: p.skills, can_shared: p.manage_skills, admin: p.manage_settings }
    }

    /// The system itself (turning off a revoked app).
    pub fn system() -> Actor {
        Actor { user: "system".into(), allowed: true, can_own: true, can_shared: true, admin: true }
    }

    pub fn default_scope(&self) -> Scope {
        if self.can_own { Scope::User(self.user.clone()) } else { Scope::Shared }
    }
}

/// What the person agreed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Confirm {
    pub permissions: bool,
    pub warnings: bool,
}

impl Confirm {
    pub fn all() -> Confirm {
        Confirm { permissions: true, warnings: true }
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::pkg::newer;
use crate::store_manifest::Meta;
use crate::store_trust::Verdict;
use iyakki_core_api::appcaps::CapabilityRequest;
use iyakki_core_api::appcaps::CardLine;
use iyakki_core_api::bundle::Member;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Perm {
    pub canonical: String,
    pub words: String,
    pub sensitive: bool,
}

/// What the person is shown before anything is installed.
#[derive(Clone, Debug)]
pub struct Review {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub title: String,
    pub meta: Meta,
    pub verdict: Verdict,
    pub steps: Vec<Step>,
    pub permissions: Vec<Perm>,
    /// The core capabilities (device and system access) the app asks for, as in its `app.json`.
    pub core_requests: Vec<CapabilityRequest>,
    /// The same, worded for the card, with the hard asks and (on an update) the new ones marked.
    pub core_lines: Vec<CardLine>,
    /// A bundle's members, grouped for the card (docs/development/CORE-ACCESS-MODEL.md section 6d); empty for an app without `contents`.
    pub members: Vec<crate::store_bundle::MemberCard>,
    /// The refusal is "needs a newer IYAKKI" (the app's `requires` or manifest version is ahead of this IYAKKI).
    pub needs_newer_iyakki: bool,
    pub warnings: Vec<String>,
    pub scope: Scope,
    /// Why it cannot be installed, if it cannot.
    pub refusal: Option<String>,
    /// A paid app without a valid licence.
    pub locked: bool,
    pub installed: Option<String>,
    /// A short look at what is inside (the text of a skill, the command of a server).
    pub preview: Vec<String>,
    pub sha256: String,
    pub size: usize,
    /// The bundle members that will be installed, with their files (not shown; `install` uses it).
    pub member_install: Vec<(Member, Vec<u8>)>,
    pub input: Input,
}

impl Review {
    pub fn can_install(&self) -> bool {
        self.refusal.is_none()
    }

    /// Whether the person must accept something beyond reading the permissions.
    pub fn needs_warning_ack(&self) -> bool {
        !self.warnings.is_empty() || self.verdict.level.needs_warning_ack()
    }

    pub fn is_sensitive(&self) -> bool {
        self.permissions.iter().any(|p| p.sensitive) || self.core_lines.iter().any(|l| l.hard_ask) || self.members.iter().any(|m| m.lines.iter().any(|l| l.hard_ask))
    }

    /// The app or one of its members asks for device and system access, so the person must answer the card.
    pub fn asks_core(&self) -> bool {
        !self.core_lines.is_empty() || self.members.iter().any(|m| !m.lines.is_empty())
    }

    /// The whole review as text: the approval prompt of the assistant, `iyakki pkg verify`, and tests.
    pub fn summary(&self) -> String {
        let mut s = format!("{} {} {}{}\n", self.kind, self.title_or_name(), self.version, match &self.installed {
            Some(v) if newer(&self.version, v) => format!(" (update from {v})"),
            Some(v) => format!(" (installed: {v})"),
            None => " (new)".into(),
        });
        s.push_str(&format!("{}: {}\n", self.verdict.level.label(), self.verdict.label));
        if let Some(p) = &self.verdict.publisher {
            s.push_str(&format!("publisher: {}\n", p.name));
        }
        for st in &self.steps {
            s.push_str(&format!("{} {}{}\n", if st.ok { "ok " } else { "NO " }, st.label, if st.note.is_empty() { String::new() } else { format!(": {}", st.note) }));
        }
        if self.permissions.is_empty() {
            s.push_str("permissions: none\n");
        } else {
            s.push_str(&format!("permissions: {}\n", self.permissions.iter().map(|p| p.words.clone()).collect::<Vec<_>>().join("; ")));
        }
        if !self.core_lines.is_empty() {
            s.push_str(&format!("device and system access: {}\n", self.core_lines.iter().map(|l| format!("{}{}{}", if l.is_new && self.installed.is_some() { "NEW " } else { "" }, l.words, if l.hard_ask { " (every use asks)" } else { "" })).collect::<Vec<_>>().join("; ")));
        }
        for m in &self.members {
            let state = match &m.state {
                crate::store_bundle::MemberState::Install => String::new(),
                crate::store_bundle::MemberState::Skip(t) => format!(" ({t}, left out)"),
            };
            s.push_str(&format!("member {} [{}]{state}{}\n", m.name(), m.kind, if m.lines.is_empty() { String::new() } else { format!(": {}", m.lines.iter().map(|l| format!("{}{}{}", if l.is_new && self.installed.is_some() { "NEW " } else { "" }, l.words, if l.hard_ask { " (every use asks)" } else { "" })).collect::<Vec<_>>().join("; ")) }));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        if let Some(r) = &self.refusal {
            s.push_str(&format!("cannot be installed: {r}\n"));
        }
        s
    }

    pub fn title_or_name(&self) -> &str {
        if self.title.is_empty() { &self.name } else { &self.title }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The review said no.
    Refused(String),
    /// The person has not yet agreed to the permissions or the warnings.
    NeedsConfirmation(String),
    /// What was reviewed is not what is there now.
    Changed(String),
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Refused(m) | InstallError::NeedsConfirmation(m) | InstallError::Changed(m) | InstallError::Failed(m) => write!(f, "{m}"),
        }
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// One thing the store checked, with its result in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub label: String,
    pub ok: bool,
    pub note: String,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::store_catalog::Entry;

#[derive(Clone, Debug)]
pub struct Input {
    pub bytes: Vec<u8>,
    pub entry: Option<Entry>,
    pub source: String,
    pub builtin: bool,
    pub catalog_expired: bool,
    pub known: Vec<(String, String)>,
    /// Going back to the previous version on purpose (the version rules for going up do not apply; the store records it).
    pub rollback: bool,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde::Deserialize;
use serde::Serialize;

/// An installed app, as the store remembers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: String,
    pub sha256: String,
    pub signer: Option<String>,
    pub cert: Option<String>,
    pub level: String,
    pub publisher: String,
    pub scope: String,
    pub source: String,
    pub installed_at: i64,
    pub disabled: bool,
    /// Came with the system image (seeded or built in).
    pub preinstalled: bool,
}

impl Record {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.kind, self.scope, self.name)
    }

    pub fn scope(&self) -> Scope {
        scope_from_key(&self.scope)
    }
}

pub fn scope_from_key(k: &str) -> Scope {
    k.strip_prefix("user/").map_or(Scope::Shared, |u| Scope::User(u.into()))
}
