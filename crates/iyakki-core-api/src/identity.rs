//! Who is asking and what this machine has.

use serde::{Deserialize, Serialize};

/// What the machine can do at all; a tile hides what is missing instead of failing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub audio: bool,
    pub display: bool,
    pub camera: bool,
    pub keyboard: bool,
    pub battery: bool,
    pub bluetooth: bool,
}

impl Capabilities {
    pub fn none() -> Capabilities {
        Capabilities::default()
    }
    pub fn all() -> Capabilities {
        Capabilities { audio: true, display: true, camera: true, keyboard: true, battery: true, bluetooth: true }
    }
    /// The names of what is present, in a fixed order (for logs and the connector's hello).
    pub fn names(&self) -> Vec<&'static str> {
        let pairs = [("audio", self.audio), ("display", self.display), ("camera", self.camera), ("keyboard", self.keyboard), ("battery", self.battery), ("bluetooth", self.bluetooth)];
        pairs.into_iter().filter(|p| p.1).map(|p| p.0).collect()
    }
}

/// The caller of an API method. Local tiles use the signed-in person; a remote UI uses the paired computer's scopes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub user: String,
    pub admin: bool,
    /// Scopes such as `sysinfo:read`; empty for a local session, which is limited by the person's role instead.
    pub scopes: Vec<String>,
}

impl Identity {
    pub fn local(user: &str, admin: bool) -> Identity {
        Identity { user: user.to_string(), admin, scopes: vec![] }
    }
    pub fn remote(user: &str, scopes: &[&str]) -> Identity {
        Identity { user: user.to_string(), admin: false, scopes: scopes.iter().map(|s| s.to_string()).collect() }
    }
    /// A local identity may do what its role allows; a remote one only what its scopes name.
    pub fn allows(&self, scope: &str) -> bool {
        (self.scopes.is_empty() && !self.user.is_empty()) || self.scopes.iter().any(|s| s == scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_list_only_what_is_present_in_a_fixed_order() {
        assert!(Capabilities::none().names().is_empty());
        assert_eq!(Capabilities::all().names(), ["audio", "display", "camera", "keyboard", "battery", "bluetooth"]);
        let c = Capabilities { bluetooth: true, audio: true, ..Capabilities::none() };
        assert_eq!(c.names(), ["audio", "bluetooth"]);
    }

    #[test]
    fn a_local_identity_is_open_and_a_remote_one_needs_the_scope() {
        assert!(Identity::local("amy", false).allows("sysinfo:read"));
        let r = Identity::remote("laptop", &["sysinfo:read"]);
        assert!(r.allows("sysinfo:read"));
        assert!(!r.allows("audio"));
        assert!(!Identity::default().allows("sysinfo:read"), "nobody is not allowed");
    }

    #[test]
    fn serde_round_trips() {
        let c = Capabilities::all();
        assert_eq!(serde_json::from_str::<Capabilities>(&serde_json::to_string(&c).unwrap()).unwrap(), c);
        let i = Identity::remote("x", &["a", "b"]);
        assert_eq!(serde_json::from_str::<Identity>(&serde_json::to_string(&i).unwrap()).unwrap(), i);
    }
}
