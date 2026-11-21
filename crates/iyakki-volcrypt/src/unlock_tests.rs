use zeroize::Zeroizing;

use crate::container::{container_len, MemStore, Volume};
use crate::header::SlotKind;
use crate::header_tests::{TestRandom, CHEAP};
use crate::unlock::*;

const PAYLOAD: u64 = 16 * 512;

fn volume(password: &[u8]) -> MemStore {
    let v = Volume::format(MemStore::new(container_len(PAYLOAD) as usize), &mut TestRandom(3), 512, PAYLOAD, password, SlotKind::Person, 1, CHEAP).unwrap();
    v.into_store()
}

struct Typed(Vec<Option<&'static str>>);
impl LineInput for Typed {
    fn read_secret(&mut self, _prompt: &str) -> Option<Zeroizing<Vec<u8>>> {
        if self.0.is_empty() {
            return None;
        }
        self.0.remove(0).map(|s| Zeroizing::new(s.as_bytes().to_vec()))
    }
}

struct Stick(Option<&'static str>);
impl StickFiles for Stick {
    fn read(&mut self, label: &str, path: &str) -> Option<Zeroizing<Vec<u8>>> {
        assert_eq!((label, path), ("IYAKKI-KEY", "disk.key"));
        self.0.map(|s| Zeroizing::new(s.as_bytes().to_vec()))
    }
}

struct Chip(Result<&'static str, &'static str>);
impl TpmBackend for Chip {
    fn unseal(&mut self) -> Result<Zeroizing<Vec<u8>>, String> {
        self.0.map(|s| Zeroizing::new(s.as_bytes().to_vec())).map_err(|e| e.to_string())
    }
}

#[test]
fn the_setting_defaults_to_the_keyboard_and_a_stick_or_tpm_always_falls_back_to_it() {
    assert_eq!(Kind::parse(""), Kind::Keyboard);
    assert_eq!(Kind::parse("garbage"), Kind::Keyboard);
    assert_eq!(Kind::parse("keyfile"), Kind::KeyFile);
    assert_eq!(Kind::parse("usb-key"), Kind::KeyFile);
    assert_eq!(Kind::parse(" TPM "), Kind::Tpm);
    assert_eq!(order(Kind::Keyboard), [Kind::Keyboard]);
    assert_eq!(order(Kind::Tpm), [Kind::Tpm, Kind::Keyboard]);
    assert_eq!(order(Kind::KeyFile), [Kind::KeyFile, Kind::Keyboard]);
    assert_eq!(Kind::Tpm.word(), "tpm");
}

#[test]
fn the_keyboard_gets_a_limited_number_of_tries_and_a_right_answer_opens_the_volume() {
    let store = volume(b"right words");
    let mut kb = Keyboard { input: Typed(vec![Some("wrong"), Some("also wrong"), Some("right words")]) };
    let o = unlock(&store, &mut [&mut kb], 3).ok().unwrap();
    assert_eq!(o.source, "keyboard");
    let mut kb = Keyboard { input: Typed(vec![Some("a"), Some("b"), Some("right words")]) };
    assert_eq!(unlock(&store, &mut [&mut kb], 2).err(), Some(Failure::TooManyTries(2)), "the third try is never offered");
}

#[test]
fn an_empty_line_or_no_keyboard_ends_in_words_not_in_a_guess() {
    let store = volume(b"pw");
    let mut kb = Keyboard { input: Typed(vec![Some("")]) };
    match unlock(&store, &mut [&mut kb], 3).err() {
        Some(Failure::NoSource(n)) => assert_eq!(n, ["keyboard: cancelled"]),
        other => panic!("{other:?}"),
    }
    let mut kb = Keyboard { input: Typed(vec![]) };
    match unlock(&store, &mut [&mut kb], 3).err() {
        Some(Failure::NoSource(n)) => assert!(n[0].contains("no keyboard")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_key_stick_opens_unattended_and_a_missing_stick_hands_over_to_the_keyboard() {
    let store = volume(b"stick secret");
    let mut stick = KeyFile { files: Stick(Some("stick secret")), label: "IYAKKI-KEY".into(), path: "disk.key".into() };
    let mut kb = Keyboard { input: Typed(vec![]) };
    assert_eq!(unlock(&store, &mut [&mut stick, &mut kb], 3).ok().unwrap().source, "key file");
    let mut gone = KeyFile { files: Stick(None), label: "IYAKKI-KEY".into(), path: "disk.key".into() };
    let mut kb = Keyboard { input: Typed(vec![Some("stick secret")]) };
    assert_eq!(unlock(&store, &mut [&mut gone, &mut kb], 3).ok().unwrap().source, "keyboard");
    // a stick with the wrong file is not retried with the same bytes, and the keyboard still gets its tries
    let mut wrong = KeyFile { files: Stick(Some("old file")), label: "IYAKKI-KEY".into(), path: "disk.key".into() };
    let mut kb = Keyboard { input: Typed(vec![Some("stick secret")]) };
    assert_eq!(unlock(&store, &mut [&mut wrong, &mut kb], 3).ok().unwrap().source, "keyboard");
}

#[test]
fn the_tpm_that_will_not_release_the_secret_never_opens_anything_and_says_why() {
    let store = volume(b"sealed");
    let mut ok = Tpm { tpm: Chip(Ok("sealed")) };
    assert_eq!(unlock(&store, &mut [&mut ok], 3).ok().unwrap().source, "TPM");
    let mut changed = Tpm { tpm: Chip(Err("the boot measurements changed")) };
    match unlock(&store, &mut [&mut changed], 3).err() {
        Some(Failure::NoSource(n)) => assert_eq!(n, ["TPM: the boot measurements changed"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_header_that_is_not_ours_is_reported_at_once_by_any_source() {
    let junk = MemStore::new(container_len(PAYLOAD) as usize);
    let mut kb = Keyboard { input: Typed(vec![Some("x")]) };
    assert!(matches!(unlock(&junk, &mut [&mut kb], 3).err(), Some(Failure::Volume(crate::VolError::NotOurs))));
}
