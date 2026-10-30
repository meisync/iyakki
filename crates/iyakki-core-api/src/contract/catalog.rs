//! The call catalogue as data: one table that the hello answer, the fakes, the docs test and (later) generated web types read, so
//! they cannot drift apart. Domains not built yet are listed in [`PLANNED`] in slice order.

use super::context::{Risk, Scope};
use super::error::RateLimit;
use super::hello::{DomainSupport, Hello, CONTRACT_VERSION};
use crate::identity::Capabilities;

pub struct CallSpec {
    pub name: &'static str,
    /// The scope a remote caller needs ([`Scope::LOCAL`] for local-only calls, which no remote scope can unlock).
    pub scope: &'static str,
    pub risk: Risk,
}

pub struct DomainSpec {
    pub name: &'static str,
    pub since_minor: u16,
    pub calls: &'static [CallSpec],
    pub events: &'static [&'static str],
}

const fn c(name: &'static str, scope: &'static str, risk: Risk) -> CallSpec {
    CallSpec { name, scope, risk }
}

pub const DOMAINS: &[DomainSpec] = &[
    DomainSpec {
        name: "settings",
        since_minor: 1,
        calls: &[c("schema", Scope::SETTINGS_READ, Risk::Read), c("get", Scope::SETTINGS_READ, Risk::Read), c("set", Scope::SETTINGS_WRITE, Risk::Change), c("watch", Scope::SETTINGS_READ, Risk::Read)],
        events: &["changed"],
    },
    DomainSpec {
        name: "vault",
        since_minor: 1,
        calls: &[c("status", Scope::VAULT_STATUS, Risk::Read), c("unlock", Scope::LOCAL, Risk::LocalOnly), c("lock", Scope::VAULT_LOCK, Risk::Change), c("use_secret", Scope::VAULT_USE, Risk::Change)],
        events: &[],
    },
    DomainSpec {
        name: "flows",
        since_minor: 1,
        calls: &[
            c("list", Scope::FLOWS_READ, Risk::Read),
            c("status", Scope::FLOWS_READ, Risk::Read),
            c("events", Scope::FLOWS_READ, Risk::Read),
            c("start", Scope::FLOWS_RUN, Risk::Change),
            c("stop", Scope::FLOWS_RUN, Risk::Change),
            c("approve", Scope::FLOWS_APPROVE, Risk::AskPerson),
        ],
        events: &["started", "step", "approval_needed", "approval_answered", "frame", "finished"],
    },
    DomainSpec {
        name: "consent",
        since_minor: 1,
        calls: &[c("pending", Scope::CONSENT_READ, Risk::Read), c("answer", Scope::LOCAL, Risk::LocalOnly)],
        events: &["asked", "answered", "expired"],
    },
];

/// Domains planned, in slice order (docs/development/CORE-CONTRACT.md). Not in `hello` until built.
pub const PLANNED: &[&str] = &[
    "accounts", "chat", "models", "store", "apps", "updates", "downloads", "files", "sharing", "backup", "cluster", "git", "network", "firewall", "browser",
    "media", "doctor", "diagnostics", "inbox", "notifications", "power", "time", "display",
];

pub fn domain(name: &str) -> Option<&'static DomainSpec> {
    DOMAINS.iter().find(|d| d.name == name)
}

pub fn call(domain_name: &str, call_name: &str) -> Option<&'static CallSpec> {
    domain(domain_name)?.calls.iter().find(|c| c.name == call_name)
}

/// The `hello` for this build.
pub fn hello_for(machine: &str, build: &str, capabilities: Capabilities) -> Hello {
    Hello {
        contract: CONTRACT_VERSION,
        oldest_minor: 1,
        machine: machine.to_string(),
        build: build.to_string(),
        domains: DOMAINS
            .iter()
            .map(|d| DomainSupport { domain: d.name.to_string(), since_minor: d.since_minor, calls: d.calls.iter().map(|c| c.name.to_string()).collect(), events: d.events.iter().map(|e| e.to_string()).collect() })
            .collect(),
        capabilities,
        rate_limit: RateLimit { calls: 120, per_ms: 60_000 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_local_only_call_has_the_local_marker_and_no_other_call_does() {
        for d in DOMAINS {
            for c in d.calls {
                assert_eq!(c.risk == Risk::LocalOnly, c.scope == Scope::LOCAL, "{}.{}", d.name, c.name);
            }
        }
    }

    #[test]
    fn planned_domains_are_not_built_ones_and_names_are_unique() {
        for p in PLANNED {
            assert!(domain(p).is_none(), "{p}");
        }
        let mut names: Vec<&str> = DOMAINS.iter().map(|d| d.name).chain(PLANNED.iter().copied()).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn every_built_domain_is_in_the_contract_document() {
        let doc = include_str!("../../../../docs/development/CORE-CONTRACT.md");
        for d in DOMAINS {
            for c in d.calls {
                assert!(doc.contains(&format!("`{}.{}`", d.name, c.name)), "CORE-CONTRACT.md lacks `{}.{}`", d.name, c.name);
            }
        }
        for p in PLANNED {
            assert!(doc.contains(&format!("`{p}`")), "CORE-CONTRACT.md lacks the planned domain `{p}`");
        }
    }
}
