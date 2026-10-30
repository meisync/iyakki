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

/// The contract this build speaks. 0.x while the contract is young: the major number stays 0 and minors may still add only.
pub const CONTRACT_VERSION: ContractVersion = ContractVersion { major: 0, minor: 1 };

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
    Negotiated { version, usable, missing }
}
