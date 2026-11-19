//! What a client device is allowed to ask for: grant, hold, revoke, and what the broker says at its next call.

use super::*;
use iyakki_core_api::broker::{Call, Denial};

fn data(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-devperm-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_names_map_to_broker_capabilities_and_only_those_can_be_given() {
    assert_eq!(permission_capability("audio.out"), Some("speaker.play"));
    assert_eq!(permission_capability("camera.view"), Some("camera.view"));
    assert_eq!(permission_capability("files.read"), None);
    assert!(DEVICE_PERMISSIONS.iter().all(|(_, c, _)| GRANTABLE.contains(c)) && DEVICE_PERMISSIONS.len() == GRANTABLE.len());
    let d = data("names");
    assert!(grant_device(&d, "dev", "admin", &["files.read"], 1).is_err());
    assert!(revoke_device(&d, "dev", &["files.read"]).is_err());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn grant_hold_revoke_and_grant_again_take_effect_at_the_next_broker_call() {
    let d = data("cycle");
    let (broker, _log) = crate::accessbroker::local(&d, None);
    let caller = Caller { app: "client".into(), kind: CallerKind::Client { device_id: "dev1".into() }, person: "ann".into(), device: "Phone".into() };
    let call = Call { caller: &caller, capability: "camera.view", unit: Some("usb:video0"), label: "cam" };
    assert!(device_holds(&d, "dev1").is_empty());
    assert_eq!(broker.decide(&call), Err(Denial::NotGranted));
    grant_device(&d, "dev1", "admin", &["camera.view", "speaker.play"], 5).unwrap();
    assert_eq!(device_holds(&d, "dev1"), vec!["speaker.play", "camera.view"]);
    assert_eq!(broker.decide(&call), Ok(()));
    assert_eq!(revoke_device(&d, "dev1", &["camera.view"]).unwrap(), 1);
    assert_eq!(revoke_device(&d, "dev1", &["camera.view"]).unwrap(), 0, "already off");
    assert_eq!(revoke_device(&d, "other", &["camera.view"]).unwrap(), 0, "a device with no book entry");
    assert_eq!(broker.decide(&call), Err(Denial::Revoked), "the same broker refuses at its next call");
    assert_eq!(device_holds(&d, "dev1"), vec!["speaker.play"]);
    grant_device(&d, "dev1", "admin", &["camera.view"], 6).unwrap();
    assert_eq!(broker.decide(&call), Ok(()));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn on_means_allowed_to_ask_a_hard_ask_still_needs_the_card_every_use() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count(AtomicUsize);
    impl CardAsk for Count {
        fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
            self.0.fetch_add(1, Ordering::SeqCst);
            CardAnswer::Allow
        }
    }
    let d = data("hard");
    let card = Arc::new(Count(AtomicUsize::new(0)));
    let (broker, _log) = crate::accessbroker::local(&d, Some(card.clone()));
    let caller = Caller { app: "client".into(), kind: CallerKind::Client { device_id: "dev1".into() }, person: "ann".into(), device: "Phone".into() };
    grant_device(&d, "dev1", "admin", &["client.camera.offer"], 5).unwrap();
    let call = Call { caller: &caller, capability: "client.camera.offer", unit: None, label: "cam" };
    for _ in 0..3 {
        assert_eq!(broker.decide(&call), Ok(()));
    }
    assert_eq!(card.0.load(Ordering::SeqCst), 3, "asked at every use although the permission is on");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_role_grants_what_it_has_and_takes_the_rest_away_and_the_broker_follows() {
    let d = data("roles");
    let (broker, _log) = crate::accessbroker::local(&d, None);
    let caller = Caller { app: "client".into(), kind: CallerKind::Client { device_id: "dev9".into() }, person: "ann".into(), device: "Phone".into() };
    let cam = Call { caller: &caller, capability: "camera.view", unit: Some("usb:video0"), label: "cam" };
    assert_eq!(role_of(&d, "dev9"), "viewer", "a new device may ask for nothing");
    assert!(apply_role(&d, "dev9", "root", "admin", 1).unwrap_err().contains("not a role"));
    apply_role(&d, "dev9", "full", "admin", 2).unwrap();
    assert_eq!(role_of(&d, "dev9"), "full");
    assert_eq!(broker.decide(&cam), Ok(()));
    apply_role(&d, "dev9", "listener", "admin", 3).unwrap();
    assert_eq!(device_holds(&d, "dev9"), vec!["speaker.play"]);
    assert_eq!(role_of(&d, "dev9"), "listener");
    assert_eq!(broker.decide(&cam), Err(Denial::Revoked), "the next call is refused");
    // one switch set by hand leaves the table: the role is named custom, not guessed
    set_permission(&d, "dev9", "clipboard.client", true, "admin", 4).unwrap();
    assert_eq!(role_of(&d, "dev9"), "presenter");
    set_permission(&d, "dev9", "camera.view", true, "admin", 5).unwrap();
    assert_eq!(role_of(&d, "dev9"), "custom");
    apply_role(&d, "dev9", "viewer", "admin", 6).unwrap();
    assert!(device_holds(&d, "dev9").is_empty());
    assert!(DEVICE_ROLES.iter().all(|r| r.2.iter().all(|c| GRANTABLE.contains(c))));
    let _ = std::fs::remove_dir_all(d);
}
