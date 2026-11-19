//! F-BOOT-12: the formats of a direct key enrolment and the verdict about the firmware. Structure only; whether a firmware
//! accepts the write is a hardware question.

use crate::sbenroll::*;
use std::collections::BTreeMap;

struct Vars(BTreeMap<String, Vec<u8>>);
impl VarSource for Vars {
    fn read(&self, name: &str, guid: &str) -> Option<Vec<u8>> {
        self.0.get(&format!("{name}-{guid}")).cloned()
    }
}
fn vars(setup: Option<u8>, secure: Option<u8>) -> Vars {
    let mut m = BTreeMap::new();
    if let Some(s) = setup {
        m.insert(format!("SetupMode-{GLOBAL_GUID}"), vec![s]);
    }
    if let Some(s) = secure {
        m.insert(format!("SecureBoot-{GLOBAL_GUID}"), vec![s]);
    }
    Vars(m)
}

fn cert() -> Vec<u8> {
    let mut c = vec![0x30, 0x82, 0x01, 0x00];
    c.extend(std::iter::repeat_n(0xAB, 256));
    c
}

#[test]
fn a_guid_is_written_with_its_first_three_fields_little_endian() {
    let g = guid_bytes("8be4df61-93ca-11d2-aa0d-00e098032b8c").unwrap();
    assert_eq!(g, [0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
    assert!(guid_bytes("nope").is_err());
}

#[test]
fn the_signature_list_has_the_sizes_the_firmware_checks() {
    let c = cert();
    let l = signature_list(&c, "11111111-2222-3333-4444-555555555555").unwrap();
    let u32at = |o: usize| u32::from_le_bytes(l[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(u32at(16), l.len(), "SignatureListSize");
    assert_eq!(u32at(20), 0, "SignatureHeaderSize");
    assert_eq!(u32at(24), 16 + c.len(), "SignatureSize");
    assert_eq!(&l[44..], &c[..], "the certificate follows the owner GUID");
    assert_eq!(&l[..16], &guid_bytes("a5c059a1-94e4-4aa7-87b5-ab155c2bf072").unwrap(), "X.509 type");
    assert!(signature_list(b"-----BEGIN CERTIFICATE-----", "11111111-2222-3333-4444-555555555555").is_err());
}

#[test]
fn what_is_signed_and_the_auth_file_follow_the_time_based_layout() {
    let t = efi_time(2026, 11, 18, 10, 0, 0);
    let payload = vec![1u8, 2, 3];
    let s = data_to_sign(Var::Db, &t, &payload).unwrap();
    // "db" in UTF-16, the db GUID, attributes 0x27, the time, the payload
    assert_eq!(&s[..4], &[b'd', 0, b'b', 0]);
    assert_eq!(&s[4..20], &guid_bytes(DB_GUID).unwrap());
    assert_eq!(&s[20..24], &0x27u32.to_le_bytes());
    assert_eq!(&s[24..40], &t);
    assert_eq!(&s[40..], &payload[..]);
    let sig = vec![0x30, 0x03, 1, 2, 3];
    let a = auth_file(&t, &sig, &payload).unwrap();
    assert_eq!(&a[..16], &t);
    assert_eq!(u32::from_le_bytes(a[16..20].try_into().unwrap()) as usize, 24 + sig.len());
    assert_eq!(&a[20..24], &[0x00, 0x02, 0xF1, 0x0E], "revision 0x0200, type 0x0EF1");
    assert_eq!(&a[a.len() - 3..], &payload[..]);
    assert!(auth_file(&t, b"junk", &payload).is_err());
    // the time is part of the signed bytes: another time means another message
    assert_ne!(s, data_to_sign(Var::Db, &efi_time(2026, 11, 19, 0, 0, 0), &payload).unwrap());
}

#[test]
fn only_setup_mode_is_open_and_every_other_state_says_what_to_do() {
    assert_eq!(judge(&vars(Some(1), Some(0)), true), Verdict::SetupMode);
    assert_eq!(judge(&vars(Some(0), Some(1)), true), Verdict::UserModeSecureBoot);
    assert_eq!(judge(&vars(Some(0), Some(0)), true), Verdict::UserMode);
    assert_eq!(judge(&vars(None, None), true), Verdict::NoUefi);
    assert_eq!(judge(&vars(Some(1), Some(0)), false), Verdict::NoUefi);
    for v in [Verdict::NoUefi, Verdict::UserMode, Verdict::UserModeSecureBoot, Verdict::SetupMode] {
        assert!(v.words().len() > 40);
    }
    let l = enrolment_lists(&cert(), "11111111-2222-3333-4444-555555555555").unwrap();
    assert_eq!(l.iter().map(|(v, _)| v.name()).collect::<Vec<_>>(), ["PK", "KEK", "db"]);
}
