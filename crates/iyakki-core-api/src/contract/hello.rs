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

/// The contract this build speaks. 0.2 adds the compact chunk encoding (`stream/read` with `"encoding": "base64"`; F-CORE-18). 0.3 adds the `appevents` domain (app events for a client device, F-SDK-202). 0.x while the contract is young: the major number stays 0 and minors may still add only.
pub const CONTRACT_VERSION: ContractVersion = ContractVersion { major: 0, minor: 3 };

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::UpdateSide;
    use crate::contract::error::RateLimit;
    use crate::identity::Capabilities;

    fn dom(name: &str, since_minor: u16, calls: &[&str]) -> DomainSupport {
        DomainSupport { domain: name.into(), since_minor, calls: calls.iter().map(|c| c.to_string()).collect(), events: vec![] }
    }

    /// A machine at `major.minor` that still serves minors from `oldest`. It has `settings` (since minor 0) and `flows` (since minor 2).
    fn machine(major: u16, minor: u16, oldest: u16) -> Hello {
        Hello {
            contract: ContractVersion { major, minor },
            oldest_minor: oldest,
            machine: "test-machine".into(),
            build: "test-build".into(),
            domains: vec![dom("settings", 0, &["get", "set"]), dom("flows", 2, &["start"])],
            capabilities: Capabilities::none(),
            rate_limit: RateLimit { calls: 10, per_ms: 1000 },
            deprecations: vec![],
        }
    }

    fn client(major: u16, minor: u16, wants: &[&str]) -> ClientHello {
        ClientHello { contract: ContractVersion { major, minor }, wants: wants.iter().map(|w| w.to_string()).collect() }
    }

    #[test]
    fn a_client_on_the_same_minor_agrees_on_it_and_every_wanted_domain_is_usable() {
        let (m, c) = (machine(0, 3, 1), client(0, 3, &["settings", "flows"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 3 }));
        assert_eq!(n.usable, ["settings", "flows"]);
        assert!(n.missing.is_empty());
        assert_eq!(n.update, None);
        assert_eq!(n.say(&m, &c), None, "nothing to say when all is well");
    }

    #[test]
    fn an_older_client_that_the_machine_still_serves_gets_the_lower_minor() {
        let (m, c) = (machine(0, 3, 1), client(0, 2, &["settings", "flows"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 2 }));
        assert_eq!(n.usable, ["settings", "flows"], "flows arrived in minor 2, which the agreed minor includes");
        assert_eq!(n.update, None);
    }

    #[test]
    fn a_client_at_exactly_the_oldest_served_minor_is_still_served() {
        let n = negotiate(&machine(0, 3, 2), &client(0, 2, &[]));
        assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 2 }));
        assert_eq!(n.update, None);
    }

    #[test]
    fn a_client_one_below_the_oldest_served_minor_gets_no_calls_and_the_client_must_update() {
        let (m, c) = (machine(0, 3, 2), client(0, 1, &["settings"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.version, None);
        assert!(n.usable.is_empty());
        assert_eq!(n.missing, ["settings"]);
        assert_eq!(n.update, Some(UpdateSide::Client));
        assert_eq!(n.say(&m, &c).as_deref(), Some("this machine speaks contract 0.3 (from 0.2); the client speaks 0.1: update the client"));
    }

    #[test]
    fn an_older_major_version_gets_no_calls_and_the_client_updates() {
        let n = negotiate(&machine(1, 0, 0), &client(0, 9, &[]));
        assert_eq!(n.version, None);
        assert_eq!(n.update, Some(UpdateSide::Client));
    }

    #[test]
    fn a_newer_major_version_on_the_client_means_the_machine_must_update() {
        let n = negotiate(&machine(0, 3, 0), &client(1, 0, &[]));
        assert_eq!(n.version, None);
        assert_eq!(n.update, Some(UpdateSide::Machine));
    }

    #[test]
    fn a_domain_added_in_a_later_minor_is_missing_even_though_the_machine_has_it() {
        let (m, c) = (machine(0, 3, 1), client(0, 1, &["flows"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 1 }));
        assert!(n.usable.is_empty());
        assert_eq!(n.missing, ["flows"]);
        assert_eq!(n.update, None, "the client is not newer than the machine, so the client simply hides the screen");
    }

    #[test]
    fn a_domain_the_machine_does_not_have_is_missing_and_its_screen_is_hidden_not_failed() {
        let (m, c) = (machine(0, 3, 1), client(0, 3, &["camera", "flows"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.usable, ["flows"]);
        assert_eq!(n.missing, ["camera"]);
        assert_eq!(n.update, None);
    }

    #[test]
    fn a_newer_client_that_wants_a_domain_the_machine_lacks_means_the_machine_must_update() {
        let (m, c) = (machine(0, 3, 1), client(0, 4, &["future"]));
        let n = negotiate(&m, &c);
        assert_eq!(n.version, Some(ContractVersion { major: 0, minor: 3 }), "the agreed minor is the lower of the two");
        assert_eq!(n.missing, ["future"]);
        assert_eq!(n.update, Some(UpdateSide::Machine));
        assert_eq!(n.say(&m, &c).as_deref(), Some("this machine speaks contract 0.3; the client speaks 0.4: update this machine to use future"));
    }

    #[test]
    fn usable_domains_keep_the_order_the_client_asked_for() {
        let n = negotiate(&machine(0, 3, 0), &client(0, 3, &["flows", "settings"]));
        assert_eq!(n.usable, ["flows", "settings"]);
    }

    #[test]
    fn a_machine_says_which_domains_it_supports_by_domain_and_call_name() {
        let m = machine(0, 3, 0);
        assert!(m.supports("settings", "get"));
        assert!(!m.supports("settings", "delete"), "an unknown call is not supported");
        assert!(!m.supports("camera", "get"), "an unknown domain is not supported");
        assert!(!m.supports("flows", "get"), "a call must belong to that domain");
    }

    #[test]
    fn a_machine_says_whether_it_has_a_domain_by_exact_name() {
        let m = machine(0, 3, 0);
        assert!(m.has_domain("flows"));
        assert!(!m.has_domain("Flows"), "names are case sensitive");
        assert!(!m.has_domain("flow"));
    }
}
