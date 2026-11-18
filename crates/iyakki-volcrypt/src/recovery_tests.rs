use crate::recovery::RecoveryKey;
use crate::{Random, VolError};

struct Fixed(u8);
impl Random for Fixed {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError> {
        for b in buf {
            self.0 = self.0.wrapping_mul(97).wrapping_add(13);
            *b = self.0;
        }
        Ok(())
    }
}

#[test]
fn a_key_prints_as_fourteen_groups_and_reads_back() {
    let k = RecoveryKey::generate(&mut Fixed(1)).unwrap();
    let t = k.to_text();
    assert_eq!(t.len(), 14 * 4 + 13);
    assert_eq!(t.split('-').count(), 14);
    let back = RecoveryKey::parse(&t).unwrap();
    assert_eq!(back.kek(), k.kek());
    assert_eq!(back.to_text(), t);
}

#[test]
fn typing_habits_are_forgiven() {
    let k = RecoveryKey::from_bytes([0xA5; 32]);
    let t = k.to_text();
    let loose = t.to_lowercase().replace('-', "  ");
    assert_eq!(RecoveryKey::parse(&loose).unwrap().kek(), k.kek());
    let all_zero = RecoveryKey::from_bytes([0; 32]);
    let z = all_zero.to_text();
    assert!(z.starts_with("0000-0000"));
    assert_eq!(RecoveryKey::parse(&z.replace('0', "o")).unwrap().kek(), all_zero.kek());
}

#[test]
fn every_single_mistyped_character_is_caught() {
    let k = RecoveryKey::generate(&mut Fixed(9)).unwrap();
    let t: Vec<char> = k.to_text().chars().collect();
    let alphabet: Vec<char> = "0123456789ABCDEFGHJKMNPQRSTVWXYZ".chars().collect();
    for (i, c) in t.iter().enumerate() {
        if *c == '-' {
            continue;
        }
        for a in &alphabet {
            let same = a == c;
            let mut u = t.clone();
            u[i] = *a;
            let s: String = u.iter().collect();
            let r = RecoveryKey::parse(&s);
            if same {
                assert!(r.is_ok());
            } else {
                assert!(r.is_err(), "position {i} {a} was accepted");
            }
        }
    }
}

#[test]
fn bad_lengths_and_characters_are_explained() {
    let t = RecoveryKey::from_bytes([1; 32]).to_text();
    assert!(matches!(RecoveryKey::parse(""), Err(VolError::BadRecoveryKey(m)) if m.contains("short")));
    assert!(matches!(RecoveryKey::parse(&t[..t.len() - 2]), Err(VolError::BadRecoveryKey(m)) if m.contains("short")));
    assert!(matches!(RecoveryKey::parse(&format!("{t}0")), Err(VolError::BadRecoveryKey(m)) if m.contains("long")));
    assert!(matches!(RecoveryKey::parse(&t.replacen(&t[..1], "U", 1)), Err(VolError::BadRecoveryKey(m)) if m.contains("character")));
    assert!(RecoveryKey::parse(&"x".repeat(100_000)).is_err());
    assert!(RecoveryKey::parse("\u{0}\u{ffff}").is_err());
}

#[test]
fn different_keys_have_different_slot_keys_and_debug_hides_the_bytes() {
    let a = RecoveryKey::from_bytes([1; 32]);
    let b = RecoveryKey::from_bytes([2; 32]);
    assert_ne!(a.kek(), b.kek());
    assert_eq!(format!("{a:?}"), "RecoveryKey(..)");
}
