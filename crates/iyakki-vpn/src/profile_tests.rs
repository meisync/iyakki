//! Tests for the per-profile settings (FULL and HYBRID).

use crate::filter::FilterSet;
use crate::profile::{Mode, Profile, Settings};
use crate::VpnError;

#[test]
fn modes_have_the_documented_defaults_and_refuse_nonsense() {
    let full = Settings::full();
    assert_eq!((full.mode, full.kill_switch, full.fail_closed, full.public_ip_lookup), (Mode::Full, false, false, false));
    assert!(full.validate().is_ok());
    let h = Settings::hybrid(FilterSet::parse("*.example.com").unwrap());
    assert_eq!((h.mode, h.kill_switch, h.fail_closed, h.public_ip_lookup), (Mode::Hybrid, false, true, false));
    assert!(h.validate().is_ok());
    // filters on a FULL profile, a kill switch on a HYBRID one, a HYBRID profile that routes nothing
    assert!(matches!(Settings { filters: FilterSet::parse("example.com").unwrap(), ..Settings::full() }.validate(), Err(VpnError::Rejected { .. })));
    assert!(matches!(Settings { kill_switch: true, ..h.clone() }.validate(), Err(VpnError::Rejected { .. })));
    assert!(matches!(Settings::hybrid(FilterSet::new()).validate(), Err(VpnError::Missing(_))));
    assert!(matches!(Settings::hybrid(FilterSet::parse("!example.com").unwrap()).validate(), Err(VpnError::Missing(_))));
}

#[test]
fn settings_are_stored_with_the_profile_and_old_profiles_load_as_full() {
    let h = Settings::hybrid(FilterSet::parse("*.example.com\n!mail.example.com\n203.0.113.0/24").unwrap());
    let json = serde_json::to_string(&h).unwrap();
    assert!(json.contains("\"mode\":\"hybrid\"") && json.contains("\"fail_closed\":true"));
    assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), h);
    // a stored list that is a catch-all does not load
    let bad = json.replace("*.example.com", "*.com");
    assert!(serde_json::from_str::<Settings>(&bad).is_err());
    // a profile written before modes existed
    let old = r#"{"id":"p1","name":"Old","body":{"kind":"openvpn","text":"client\n","remotes":[],"routes":[],"dns":[],"redirect_gateway":false,"needs_credentials":false,"secret_blocks":[]}}"#;
    let p: Profile = serde_json::from_str(old).unwrap();
    assert_eq!(p.settings, Settings::full());
}
