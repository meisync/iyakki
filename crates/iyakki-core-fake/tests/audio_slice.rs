//! The fake's side of the Audio slice (F-ARCH-06): canned cards, a volume that moves, and every call recorded.

use iyakki_core_api::{ApiError, Os};
use iyakki_core_fake::{Canned, FakeOs};

#[test]
fn the_fake_answers_cards_and_moves_its_volume_and_records_the_calls() {
    let os = FakeOs::new();
    assert_eq!(os.audio().sound_cards().unwrap()[0].name, "PCH");
    assert_eq!(os.audio().volume(0).unwrap().percent, 40);
    assert_eq!(os.audio().set_volume(0, 75).unwrap().percent, 75);
    assert_eq!(os.audio().change_volume(0, Some(-80)).unwrap().percent, 0, "cut at zero");
    assert!(os.audio().change_volume(0, None).unwrap().muted);
    assert!(!os.audio().set_volume(0, 10).unwrap().muted, "setting a level ends a mute");
    assert_eq!(os.calls(), ["sound_cards", "volume(0)", "set_volume(0,75)", "change_volume(0,Some(-80))", "change_volume(0,None)", "set_volume(0,10)"]);
    // a card without a control, as `None` canned data says
    let bare = FakeOs::with(Canned { volume: None, sound_cards: None, ..Canned::default() });
    assert!(matches!(bare.audio().volume(0), Err(ApiError::Unsupported(_))));
    assert!(matches!(bare.audio().sound_cards(), Err(ApiError::Unsupported(_))));
    // the other slices are reachable through the same handle
    assert!(!os.disks().mounts().unwrap().is_empty() && !os.net().interfaces().unwrap().is_empty());
}

#[test]
fn the_fake_also_answers_space_and_addresses_and_records_them() {
    let os = FakeOs::new();
    assert_eq!(os.disks().space("/var/lib/iyakki").unwrap(), (50_000_000_000, 100_000_000_000));
    assert!(os.net().ipv4_addresses().unwrap().iter().any(|(_, ip)| ip == "192.168.1.20"));
    assert_eq!(os.calls(), ["space(/var/lib/iyakki)", "ipv4_addresses"]);
    let none = FakeOs::with(Canned { space: None, ipv4: None, ..Canned::default() });
    assert!(none.disks().space("/").is_err() && none.net().ipv4_addresses().is_err());
}
