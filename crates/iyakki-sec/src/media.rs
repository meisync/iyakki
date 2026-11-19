//! Who may open the camera, the microphone, sound out and clipboard reading of a remote (browser) session.
//!
//! Default deny. Two switches have to agree before anything is allowed:
//! 1. the tenant (or the machine) allows browser media at all (`media: allow|deny`, deny unless set), and
//! 2. the person's role grants the device, unless an admin refined that one user.
//!
//! The role grants are small on purpose: a guest may only listen; operators, users and admins may also use the microphone, the
//! camera and read the clipboard (each still needs the person's own click in the page, and every use is audited). A per-user
//! refinement (`allow` or `deny` a device) beats the role, but never beats the tenant switch: a tenant that denies media denies
//! it for everybody, admins included. A device name that is not known is denied.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::policy::Role;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    /// The browser's microphone.
    Mic,
    /// The browser's camera.
    Camera,
    /// Sound from the session to the browser's speakers.
    Audio,
    /// Reading the browser's clipboard.
    ClipRead,
}

impl Device {
    pub const ALL: [Device; 4] = [Device::Mic, Device::Camera, Device::Audio, Device::ClipRead];

    pub fn name(self) -> &'static str {
        match self {
            Device::Mic => "mic",
            Device::Camera => "camera",
            Device::Audio => "audio",
            Device::ClipRead => "clipread",
        }
    }

    pub fn parse(s: &str) -> Option<Device> {
        Device::ALL.into_iter().find(|d| d.name() == s)
    }

    /// Devices that capture the person (their voice, their face, what they copied). These need a role grant; sound out does not capture.
    pub fn captures(self) -> bool {
        self != Device::Audio
    }
}

/// What the role grants when the tenant allows media.
pub fn role_grants(role: Role) -> BTreeSet<Device> {
    match role {
        Role::Admin | Role::Operator | Role::User => Device::ALL.into_iter().collect(),
        Role::Guest => [Device::Audio].into_iter().collect(),
    }
}

/// Why a device was or was not allowed (for the audit line and the message in the page).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    DenyTenant,
    DenyRole,
    DenyUser,
}

impl Decision {
    pub fn allowed(self) -> bool {
        self == Decision::Allow
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPolicy {
    /// The tenant switch. False (the default) denies everything.
    #[serde(default)]
    pub tenant_allow: bool,
    /// Per user refinements: devices named here are allowed although the role does not grant them.
    #[serde(default)]
    pub user_allow: BTreeMap<String, BTreeSet<Device>>,
    /// Per user refinements: devices named here are denied although the role grants them. A deny beats an allow.
    #[serde(default)]
    pub user_deny: BTreeMap<String, BTreeSet<Device>>,
}

impl MediaPolicy {
    pub fn deny_all() -> MediaPolicy {
        MediaPolicy::default()
    }

    pub fn tenant(allow: bool) -> MediaPolicy {
        MediaPolicy { tenant_allow: allow, ..MediaPolicy::default() }
    }

    /// `IYAKKI_BROWSER_MEDIA=allow|deny`: anything else (or unset) is deny.
    pub fn from_setting(v: Option<&str>) -> MediaPolicy {
        MediaPolicy::tenant(v.map(str::trim) == Some("allow"))
    }

    pub fn decide(&self, user: &str, role: Role, d: Device) -> Decision {
        if !self.tenant_allow {
            return Decision::DenyTenant;
        }
        if self.user_deny.get(user).is_some_and(|s| s.contains(&d)) {
            return Decision::DenyUser;
        }
        if self.user_allow.get(user).is_some_and(|s| s.contains(&d)) || role_grants(role).contains(&d) {
            return Decision::Allow;
        }
        Decision::DenyRole
    }

    pub fn allows(&self, user: &str, role: Role, d: Device) -> bool {
        self.decide(user, role, d).allowed()
    }

    /// The devices this person may switch on.
    pub fn devices(&self, user: &str, role: Role) -> BTreeSet<Device> {
        Device::ALL.into_iter().filter(|d| self.allows(user, role, *d)).collect()
    }

    /// Parse a policy file; anything that does not parse is the default (deny).
    pub fn parse(text: &str) -> MediaPolicy {
        serde_json::from_str(text).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_denies_everything_for_every_role() {
        let p = MediaPolicy::default();
        for r in Role::ALL {
            for d in Device::ALL {
                assert_eq!(p.decide("anyone", r, d), Decision::DenyTenant);
            }
            assert!(p.devices("anyone", r).is_empty());
        }
        assert_eq!(MediaPolicy::from_setting(None), MediaPolicy::deny_all());
        assert_eq!(MediaPolicy::from_setting(Some("maybe")), MediaPolicy::deny_all());
        assert!(MediaPolicy::from_setting(Some(" allow ")).tenant_allow);
    }

    #[test]
    fn a_tenant_that_allows_gives_roles_their_grants() {
        let p = MediaPolicy::tenant(true);
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert_eq!(p.devices("u", r).len(), 4, "{r:?}");
        }
        let guest = p.devices("g", Role::Guest);
        assert_eq!(guest.into_iter().collect::<Vec<_>>(), vec![Device::Audio], "a guest may listen only");
        assert_eq!(p.decide("g", Role::Guest, Device::Mic), Decision::DenyRole);
        assert_eq!(p.decide("g", Role::Guest, Device::Camera), Decision::DenyRole);
    }

    #[test]
    fn user_refinements_beat_the_role_but_not_the_tenant() {
        let mut p = MediaPolicy::tenant(true);
        p.user_deny.insert("kim".into(), [Device::Camera].into_iter().collect());
        p.user_allow.insert("guest1".into(), [Device::Mic, Device::Camera].into_iter().collect());
        p.user_allow.insert("kim".into(), [Device::Camera].into_iter().collect());
        assert_eq!(p.decide("kim", Role::Admin, Device::Camera), Decision::DenyUser, "a deny beats an allow, even for an admin");
        assert!(p.allows("kim", Role::Admin, Device::Mic));
        assert!(p.allows("guest1", Role::Guest, Device::Mic) && p.allows("guest1", Role::Guest, Device::Camera));
        assert!(!p.allows("guest1", Role::Guest, Device::ClipRead));
        assert!(!p.allows("other", Role::Guest, Device::Mic), "refinements are per user");
        p.tenant_allow = false;
        assert!(!p.allows("guest1", Role::Guest, Device::Mic) && !p.allows("kim", Role::Admin, Device::Mic), "the tenant switch beats every refinement");
    }

    #[test]
    fn a_file_that_does_not_parse_denies() {
        assert_eq!(MediaPolicy::parse("not json"), MediaPolicy::deny_all());
        assert_eq!(MediaPolicy::parse("{}"), MediaPolicy::deny_all());
        let p = MediaPolicy::parse(r#"{"tenant_allow":true,"user_deny":{"kim":["mic"]}}"#);
        assert!(p.allows("kim", Role::User, Device::Camera) && !p.allows("kim", Role::User, Device::Mic));
        assert_eq!(MediaPolicy::parse(r#"{"tenant_allow":true,"user_deny":{"kim":["laser"]}}"#), MediaPolicy::deny_all(), "an unknown device name spoils the file: deny");
    }

    #[test]
    fn device_names_round_trip_and_capture_is_marked() {
        for d in Device::ALL {
            assert_eq!(Device::parse(d.name()), Some(d));
        }
        assert_eq!(Device::parse("screen"), None);
        assert!(Device::Mic.captures() && Device::Camera.captures() && Device::ClipRead.captures() && !Device::Audio.captures());
    }
}
