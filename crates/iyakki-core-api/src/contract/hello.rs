//! Version and capability discovery: the first call on any transport. A newer cloud UI works with an older machine because it
//! reads what the machine says it supports instead of assuming.

use super::error::RateLimit;
use crate::identity::Capabilities;
use serde::{Deserialize, Serialize};

/// `major` changes break the wire shape (a client refuses to talk across it); `minor` only adds calls, events and fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContractVersion {
    pub major: u16,
    pub minor: u16,
}

/// The contract this build speaks. 0.2 adds the compact chunk encoding (`stream/read` with `"encoding": "base64"`; F-CORE-18). 0.x while the contract is young: the major number stays 0 and minors may still add only.
pub const CONTRACT_VERSION: ContractVersion = ContractVersion { major: 0, minor: 2 };

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainSupport {
    pub domain: String,
    /// The contract minor in which the domain first appeared.
    pub since_minor: u16,
    pub calls: Vec<String>,
    pub events: Vec<String>,
}

/// What a machine answers to `hello`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub contract: ContractVersion,
    /// The oldest minor of the same major this machine still serves.
    pub oldest_minor: u16,
    pub machine: String,
    pub build: String,
    pub domains: Vec<DomainSupport>,
    pub capabilities: Capabilities,
    pub rate_limit: RateLimit,
    /// Names this machine has deprecated (see `compat::deprecations`): a client shows them and stops using them before they go.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprecations: Vec<crate::compat::Deprecation>,
}

impl Hello {
    pub fn supports(&self, domain: &str, call: &str) -> bool {
        self.domains.iter().any(|d| d.domain == domain && d.calls.iter().any(|c| c == call))
    }
    pub fn has_domain(&self, domain: &str) -> bool {
        self.domains.iter().any(|d| d.domain == domain)
    }
}

/// What a client says about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub contract: ContractVersion,
    /// Domains the client's screens would like to use.
    pub wants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// `None` when the majors differ or the client is older than the machine still serves: no calls at all.
    pub version: Option<ContractVersion>,
    /// Wanted domains the machine has.
    pub usable: Vec<String>,
    /// Wanted domains the machine lacks: the client hides those screens, it does not fail.
    pub missing: Vec<String>,
    /// When no version could be agreed, or a wanted domain is missing because the machine is older: the side that must update.
    pub update: Option<crate::compat::UpdateSide>,
}

/// Agree on the lower of the two minors when the majors match and the client is not older than the machine's oldest served minor.
pub fn negotiate(machine: &Hello, client: &ClientHello) -> Negotiated {
    let same_major = machine.contract.major == client.contract.major;
    let served = same_major && client.contract.minor >= machine.oldest_minor;
    let version = served.then(|| ContractVersion { major: machine.contract.major, minor: machine.contract.minor.min(client.contract.minor) });
    let (mut usable, mut missing) = (vec![], vec![]);
    for w in &client.wants {
        // A domain added in a minor above the agreed one is not usable even if present.
        let ok = version.is_some_and(|v| machine.domains.iter().any(|d| &d.domain == w && d.since_minor <= v.minor));
        if ok { usable.push(w.clone()) } else { missing.push(w.clone()) }
    }
    // the side to update (charter section 5): a different major means the older major updates; the same major with a client older than
    // the machine still serves means the client; a client that wants what only a newer minor has means the machine
    let update = if !same_major {
        Some(if client.contract.major < machine.contract.major { crate::compat::UpdateSide::Client } else { crate::compat::UpdateSide::Machine })
    } else if !served {
        Some(crate::compat::UpdateSide::Client)
    } else if !missing.is_empty() && client.contract.minor > machine.contract.minor {
        Some(crate::compat::UpdateSide::Machine)
    } else {
        None
    };
    Negotiated { version, usable, missing, update }
}

impl Negotiated {
    /// One plain sentence for a refusal or a note: nothing when all is well.
    pub fn say(&self, machine: &Hello, client: &ClientHello) -> Option<String> {
        let side = self.update?;
        Some(match side {
            crate::compat::UpdateSide::Client => format!("this machine speaks contract {}.{} (from {}.{}); the client speaks {}.{}: update the client", machine.contract.major, machine.contract.minor, machine.contract.major, machine.oldest_minor, client.contract.major, client.contract.minor),
            crate::compat::UpdateSide::Machine => format!("this machine speaks contract {}.{}; the client speaks {}.{}: update this machine to use {}", machine.contract.major, machine.contract.minor, client.contract.major, client.contract.minor, self.missing.join(", ")),
        })
    }
}
