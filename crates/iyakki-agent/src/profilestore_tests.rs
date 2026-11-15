//! Browser profile protection: round trip, wrong key, vault re-wrap, tampering, crash leftovers, migration, separate people,
//! interrupted seals and "no plain file left". Fake files in temp folders only; no real profile, no real secrets.

use super::*;

const MARK: &str = "FAKE-COOKIE-VALUE-0001";

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-profstore-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(n: u8) -> ProfileKey {
    ProfileKey::from_bytes([n; 32])
}

/// A plain profile as the engine leaves it.
fn plain_profile(st: &ProfileStore, extra: &str) {
    let p = st.profile_path();
    std::fs::create_dir_all(p.join("data/storage/site")).unwrap();
    std::fs::create_dir_all(p.join("config")).unwrap();
    std::fs::write(p.join("data/cookies.sqlite"), format!("SQLite {MARK} {extra}")).unwrap();
    std::fs::write(p.join("data/storage/site/idb.bin"), vec![7u8; 3000]).unwrap();
    std::fs::write(p.join("config/prefs"), b"zoom=1").unwrap();
}

fn read(st: &ProfileStore, rel: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(st.profile_path().join(rel)).unwrap()).to_string()
}

fn all_files(d: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        let md = std::fs::symlink_metadata(&p).unwrap();
        if md.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// No file below `root` has the marker in its name or contents (links are not followed).
fn assert_no_plaintext(root: &Path) {
    let mut v = Vec::new();
    all_files(root, &mut v);
    for f in v {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        assert!(name != "cookies.sqlite" && name != "idb.bin" && name != "prefs", "plain file name left: {}", f.display());
        if std::fs::symlink_metadata(&f).unwrap().file_type().is_file() {
            let b = std::fs::read(&f).unwrap();
            assert!(!b.windows(MARK.len()).any(|w| w == MARK.as_bytes()), "plain content left in {}", f.display());
        }
    }
}

fn protected(name: &str, user: &str, k: &ProfileKey) -> (PathBuf, ProfileStore) {
    let d = temp(name);
    let st = ProfileStore::new(&d, user);
    plain_profile(&st, "a");
    st.protect(k).unwrap();
    (d, st)
}

#[test]
fn round_trip_keeps_every_file_and_leaves_no_plain_copy_after_seal() {
    let k = key(1);
    let (d, st) = protected("rt", "alice", &k);
    assert_eq!(st.status(), Status::Sealed);
    assert!(st.pre().is_dir(), "the plain copy waits as the rollback until the first open");
    assert!(!st.open(&k).unwrap());
    assert!(!st.pre().exists(), "removed after the first successful open");
    assert_eq!(st.status(), Status::Open);
    assert!(read(&st, "data/cookies.sqlite").contains(MARK));
    assert_eq!(std::fs::read(st.profile_path().join("data/storage/site/idb.bin")).unwrap(), vec![7u8; 3000]);
    // the engine's path is the same one as before; the decrypted files live in the RAM folder
    assert!(std::fs::symlink_metadata(st.profile_path()).unwrap().file_type().is_symlink());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(st.live()).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(st.live().join("data/cookies.sqlite")).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(st.profile_path().join("data/cookies.sqlite"), format!("SQLite {MARK} b")).unwrap();
    st.seal(&k).unwrap();
    assert_eq!(st.status(), Status::Sealed);
    assert!(!st.live().exists() && std::fs::symlink_metadata(st.profile_path()).is_err());
    assert_no_plaintext(&d);
    // once more, with the later change
    st.open(&k).unwrap();
    assert!(read(&st, "data/cookies.sqlite").ends_with(" b"));
}

#[test]
fn a_wrong_key_is_refused_and_never_falls_back_to_the_backup() {
    let (_d, st) = protected("wrong", "alice", &key(1));
    st.open(&key(1)).unwrap();
    st.seal(&key(1)).unwrap();
    st.open(&key(1)).unwrap();
    st.seal(&key(1)).unwrap();
    assert!(st.backup().is_file());
    assert_eq!(st.open(&key(2)), Err(ProfileError::WrongKey));
    assert_eq!(st.status(), Status::Sealed);
    assert!(!st.live().exists());
    assert_eq!(st.unprotect(&key(2)), Err(ProfileError::WrongKey));
    assert_eq!(st.rekey(&key(2), &key(3)), Err(ProfileError::WrongKey));
}

#[test]
fn a_password_change_rewraps_the_vault_and_the_sealed_profile_opens_with_the_same_key() {
    let d = temp("pw");
    let vp = d.join("vault/users/alice.afv");
    let mut v = crate::vault::LocalVault::create(&vp, "user:alice", "first-password-1").unwrap();
    let k = key_in_vault(&mut v, true).unwrap();
    assert_eq!(key_in_vault(&mut v, true).unwrap().to_hex().as_str(), k.to_hex().as_str(), "created once");
    let st = ProfileStore::new(&d, "alice");
    plain_profile(&st, "pw");
    st.protect(&k).unwrap();
    // nothing in the vault file or the sealed file shows the key
    let vault_bytes = std::fs::read(&vp).unwrap();
    assert!(!String::from_utf8_lossy(&vault_bytes).contains(k.to_hex().as_str()));
    let sealed_before = std::fs::read(st.sealed()).unwrap();
    drop(v);
    let mut v = crate::vault::LocalVault::open(&vp).unwrap();
    v.change_password("user:alice", "first-password-1", "second-password-2").unwrap();
    assert_eq!(std::fs::read(st.sealed()).unwrap(), sealed_before, "nothing re-encrypted");
    let mut v = crate::vault::LocalVault::open(&vp).unwrap();
    assert!(v.unlock("user:alice", "first-password-1").is_err(), "the old password no longer opens the vault, so not the profile");
    v.unlock("user:alice", "second-password-2").unwrap();
    let k2 = key_in_vault(&mut v, false).unwrap();
    st.open(&k2).unwrap();
    assert!(read(&st, "data/cookies.sqlite").contains(MARK));
}

#[test]
fn a_changed_byte_a_cut_file_and_a_dropped_chunk_are_detected_and_the_backup_is_used() {
    let k = key(4);
    let (_d, st) = protected("tamper", "alice", &k);
    st.open(&k).unwrap();
    // a second chunk: more than 1 MiB of (incompressible-looking) data
    std::fs::write(st.profile_path().join("data/big.bin"), (0..2_500_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect::<Vec<u8>>()).unwrap();
    st.seal(&k).unwrap(); // sealed = with big, no backup yet (first seal after the migration has no previous sealed file? it does)
    let good = std::fs::read(st.sealed()).unwrap();
    // flip one byte in the middle
    let mut b = good.clone();
    let m = b.len() / 2;
    b[m] ^= 1;
    std::fs::remove_file(st.backup()).ok();
    std::fs::write(st.sealed(), &b).unwrap();
    assert!(matches!(st.open(&k), Err(ProfileError::Damaged(_))), "{:?}", st.open(&k));
    // cut at a chunk boundary: the chunk before the cut was written as "not last"
    let mut off = 65usize;
    let mut lens = vec![];
    while off < good.len() {
        let l = u32::from_be_bytes(good[off..off + 4].try_into().unwrap()) as usize;
        off += 4 + l;
        lens.push(off);
    }
    assert!(lens.len() >= 3, "more than two chunks");
    std::fs::write(st.sealed(), &good[..lens[lens.len() - 2]]).unwrap();
    assert!(matches!(st.open(&k), Err(ProfileError::Damaged(_))), "dropping the last chunk is caught");
    // two chunks swapped
    let c0 = (65, lens[0]);
    let c1 = (lens[0], lens[1]);
    let mut sw = good[..65].to_vec();
    sw.extend_from_slice(&good[c1.0..c1.1]);
    sw.extend_from_slice(&good[c0.0..c0.1]);
    sw.extend_from_slice(&good[lens[1]..]);
    std::fs::write(st.sealed(), &sw).unwrap();
    assert!(matches!(st.open(&k), Err(ProfileError::Damaged(_))), "reordering is caught");
    // with a good backup the damaged main file is replaced by it
    std::fs::write(st.backup(), &good).unwrap();
    assert_eq!(st.open(&k), Ok(true), "backup used");
    assert_eq!(std::fs::metadata(st.profile_path().join("data/big.bin")).unwrap().len(), 2_500_000);
    // not our file at all
    st.seal(&k).unwrap();
    std::fs::write(st.sealed(), b"hello").unwrap();
    std::fs::remove_file(st.backup()).unwrap();
    assert!(matches!(st.open(&k), Err(ProfileError::Damaged(_))));
}

#[test]
fn a_crash_leaves_a_decrypted_copy_that_the_next_start_wipes() {
    let k = key(5);
    let (d, st) = protected("crash", "alice", &k);
    st.open(&k).unwrap();
    std::fs::write(st.profile_path().join("data/newer.txt"), MARK).unwrap();
    assert!(st.live().join("data/newer.txt").exists());
    // the shell dies here; a start of the shell runs wipe_stale
    let n = wipe_stale(&d);
    assert_eq!(n, 1);
    assert!(!st.live().exists());
    assert!(std::fs::symlink_metadata(st.profile_path()).is_err(), "the dangling link is gone too");
    assert_no_plaintext(&d);
    assert_eq!(st.status(), Status::Sealed);
    st.open(&k).unwrap();
    assert!(!st.profile_path().join("data/newer.txt").exists(), "changes since the last write-back are lost: documented");
    assert!(read(&st, "data/cookies.sqlite").contains(MARK));
}

#[test]
fn migration_from_plain_verifies_keeps_a_rollback_and_can_be_repeated_after_a_stop() {
    let k = key(6);
    let d = temp("mig");
    let st = ProfileStore::new(&d, "alice");
    plain_profile(&st, "m");
    // stopped right after the sealed file was placed: both exist, and the engine is blocked
    let stopped = ProfileStore::new(&d, "alice").with_fault(Step::SealedPlaced);
    assert_eq!(stopped.protect(&k), Err(ProfileError::Interrupted("sealed-placed")));
    assert_eq!(st.status(), Status::NeedsFinish);
    assert!(engine_blocked(&d, "alice").unwrap().contains("interrupted"));
    assert_eq!(st.open(&k), Err(ProfileError::NeedsFinish));
    assert!(read(&st, "data/cookies.sqlite").contains(MARK), "the plain files are untouched");
    // again
    st.protect(&k).unwrap();
    assert_eq!(st.status(), Status::Sealed);
    assert!(st.pre().join("data/cookies.sqlite").is_file(), "rollback copy");
    // rollback: the plain profile is back, protection is gone
    st.rollback().unwrap();
    assert_eq!(st.status(), Status::Off);
    assert!(read(&st, "data/cookies.sqlite").contains(MARK));
    assert!(!st.sealed().exists() && !st.pre().exists());
    // a failed verification (a file changes between copy and compare) leaves the plain profile in place
    plain_profile(&st, "again");
    st.protect(&k).unwrap();
    st.open(&k).unwrap();
    assert!(!st.pre().exists());
    assert!(read(&st, "data/cookies.sqlite").ends_with("again"));
}

#[test]
fn two_people_have_separate_keys_and_files() {
    let d = temp("two");
    let (a, b) = (ProfileStore::new(&d, "alice"), ProfileStore::new(&d, "bob"));
    plain_profile(&a, "alice");
    plain_profile(&b, "bob");
    a.protect(&key(10)).unwrap();
    b.protect(&key(11)).unwrap();
    assert_eq!(a.open(&key(11)), Err(ProfileError::WrongKey));
    assert_eq!(b.open(&key(10)), Err(ProfileError::WrongKey));
    a.open(&key(10)).unwrap();
    b.open(&key(11)).unwrap();
    assert!(read(&a, "data/cookies.sqlite").ends_with("alice"));
    assert!(read(&b, "data/cookies.sqlite").ends_with("bob"));
    // swapping the files does not help: alice's file with bob's key
    a.seal(&key(10)).unwrap();
    b.seal(&key(11)).unwrap();
    std::fs::copy(b.sealed(), a.sealed()).unwrap();
    assert_eq!(a.open(&key(10)), Err(ProfileError::WrongKey));
}

#[test]
fn an_interrupted_seal_at_any_point_leaves_the_old_sealed_copy_intact_and_the_ram_copy_kept() {
    let k = key(7);
    for step in [Step::NewWritten, Step::NewVerified, Step::BackupLinked, Step::Rotated] {
        let (d, st) = protected(&format!("kill-{step:?}"), "alice", &k);
        st.open(&k).unwrap();
        st.seal(&k).unwrap(); // a first generation exists, with a backup next time
        st.open(&k).unwrap();
        std::fs::write(st.profile_path().join("data/new.txt"), "newer").unwrap();
        let before = std::fs::read(st.sealed()).unwrap();
        let hurt = ProfileStore::new(&d, "alice").with_fault(step);
        assert!(matches!(hurt.seal(&k), Err(ProfileError::Interrupted(_))), "{step:?}");
        assert_eq!(std::fs::read(st.sealed()).unwrap(), before, "{step:?}: the sealed file is the old one, whole");
        assert!(st.live().join("data/new.txt").exists(), "{step:?}: nothing lost from the RAM copy");
        // power loss: the RAM copy is gone, the old sealed copy opens
        wipe_stale(&d);
        assert!(!st.newf().exists(), "{step:?}: the half-written file is removed at start");
        assert_eq!(st.open(&k), Ok(false), "{step:?}");
        assert!(!st.profile_path().join("data/new.txt").exists());
        assert!(read(&st, "data/cookies.sqlite").contains(MARK));
        // and a seal after the stop works
        std::fs::write(st.profile_path().join("data/new.txt"), "newer").unwrap();
        st.seal(&k).unwrap();
        st.open(&k).unwrap();
        assert_eq!(read(&st, "data/new.txt"), "newer", "{step:?}");
    }
}

#[test]
fn the_backup_rolls_forward_and_both_files_are_whole() {
    let k = key(8);
    let (_d, st) = protected("roll", "alice", &k);
    for i in 0..3 {
        st.open(&k).unwrap();
        std::fs::write(st.profile_path().join("data/gen"), format!("g{i}")).unwrap();
        st.seal(&k).unwrap();
    }
    assert!(st.sealed().is_file() && st.backup().is_file());
    // the backup is the generation before (g1), the sealed one is g2
    std::fs::remove_file(st.sealed()).unwrap();
    st.open(&k).unwrap();
    assert_eq!(read(&st, "data/gen"), "g1");
}

#[test]
fn checkpoint_writes_back_and_keeps_the_copy_open() {
    let k = key(9);
    let (_d, st) = protected("chk", "alice", &k);
    st.open(&k).unwrap();
    std::fs::write(st.profile_path().join("data/c"), "1").unwrap();
    st.checkpoint(&k).unwrap();
    assert_eq!(st.status(), Status::Open);
    st.wipe_live();
    st.open(&k).unwrap();
    assert_eq!(read(&st, "data/c"), "1");
}

#[test]
fn rekey_unprotect_and_reset() {
    let (d, st) = protected("rkr", "alice", &key(1));
    st.rekey(&key(1), &key(2)).unwrap();
    assert_eq!(st.open(&key(1)), Err(ProfileError::WrongKey));
    st.open(&key(2)).unwrap();
    assert_eq!(st.rekey(&key(2), &key(3)), Err(ProfileError::Open));
    st.seal(&key(2)).unwrap();
    st.unprotect(&key(2)).unwrap();
    assert_eq!(st.status(), Status::Off);
    assert!(read(&st, "data/cookies.sqlite").contains(MARK) && !st.sealed().exists() && !st.backup().exists());
    // protect again, then reset without any key (a forgotten password)
    st.protect(&key(4)).unwrap();
    st.open(&key(4)).unwrap();
    st.seal(&key(4)).unwrap();
    st.reset();
    assert_eq!(st.status(), Status::Off);
    assert_no_plaintext(&d);
    let mut v = Vec::new();
    all_files(&d.join("web-profile"), &mut v);
    assert!(v.is_empty(), "nothing is left: {v:?}");
}

#[test]
fn symbolic_links_inside_a_profile_are_not_followed_into_the_archive() {
    let k = key(12);
    let d = temp("sym");
    let st = ProfileStore::new(&d, "alice");
    plain_profile(&st, "s");
    let outside = d.join("outside.txt");
    std::fs::write(&outside, "OUTSIDE-SECRET").unwrap();
    std::os::unix::fs::symlink(&outside, st.profile_path().join("data/link")).unwrap();
    st.protect(&k).unwrap();
    st.open(&k).unwrap();
    assert!(!st.profile_path().join("data/link").exists());
}

#[test]
fn an_archive_with_a_path_that_leaves_the_profile_is_refused() {
    let k = key(13);
    let d = temp("evil");
    let st = ProfileStore::new(&d, "alice");
    std::fs::create_dir_all(st.profile_path()).unwrap();
    for bad in ["../escape", "/abs", "data/../../x", "other/x", ""] {
        let mut w = SealWriter::new(Vec::new(), &k).unwrap();
        w.write_all(&[K_FILE]).unwrap();
        w.write_all(&(bad.len() as u16).to_be_bytes()).unwrap();
        w.write_all(bad.as_bytes()).unwrap();
        w.write_all(&1u64.to_be_bytes()).unwrap();
        w.write_all(b"x").unwrap();
        w.write_all(&[K_END]).unwrap();
        let (bytes, _, _) = w.finish().unwrap();
        let mut r = SealReader::open(&bytes[..], &k).unwrap();
        let dest = d.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(matches!(unpack(&mut r, Mode::Extract(&dest)), Err(ProfileError::Damaged(_))), "{bad:?}");
    }
    assert!(!d.join("escape").exists() && !d.join("x").exists());
}

#[test]
fn sign_in_and_sign_out_hooks_open_and_seal_and_the_engine_is_blocked_while_sealed() {
    let k = key(14);
    let d = temp("hooks");
    let st = ProfileStore::new(&d, "alice");
    assert_eq!(on_sign_in(&d, "alice", None), SignIn::Off);
    assert!(engine_blocked(&d, "alice").is_none());
    plain_profile(&st, "h");
    st.protect(&k).unwrap();
    assert!(engine_blocked(&d, "alice").unwrap().contains("not open"));
    assert!(crate::webengine::prepare_profile(&d, "alice").is_err(), "no fresh plain profile beside the sealed one");
    assert!(!st.profile_path().exists());
    // no key (forgotten password, vault reset): locked, with a hint that never shows a key
    let r = on_sign_in(&d, "alice", None);
    assert!(matches!(&r, SignIn::Locked(m) if m.contains("reset the browser data")), "{r:?}");
    assert!(matches!(on_sign_in(&d, "alice", Some(key(99))), SignIn::Locked(_)));
    assert_eq!(on_sign_in(&d, "alice", Some(k.clone())), SignIn::Opened { from_backup: false });
    assert!(engine_blocked(&d, "alice").is_none());
    let p = crate::webengine::prepare_profile(&d, "alice").unwrap();
    assert!(p.join("data/cookies.sqlite").is_file(), "the engine finds the profile where it always did");
    assert!(p.join("cache").is_dir());
    std::fs::write(p.join("data/added"), "x").unwrap();
    assert_eq!(on_checkpoint(&d, "alice"), Ok(()));
    assert_eq!(on_sign_out(&d, "alice"), Ok(true));
    assert_eq!(on_sign_out(&d, "alice"), Ok(false), "twice is harmless");
    assert_eq!(st.status(), Status::Sealed);
    assert_eq!(summary(&d), "1 protected profile(s), 0 open");
    assert_no_plaintext(&d);
}

#[test]
fn the_key_and_status_texts_never_show_a_key() {
    let k = key(0xab);
    assert_eq!(format!("{k:?}"), "ProfileKey(..)");
    assert_eq!(ProfileKey::from_hex(&k.to_hex()).unwrap().to_hex().as_str(), k.to_hex().as_str());
    assert!(ProfileKey::from_hex("zz").is_none() && ProfileKey::from_hex(&"g".repeat(64)).is_none());
    for s in [Status::Off, Status::Sealed, Status::Open, Status::NeedsFinish] {
        assert!(!s.text().contains("abab"));
    }
    for e in [ProfileError::WrongKey, ProfileError::Open, ProfileError::TooBig] {
        assert!(!e.to_string().contains('/'), "no paths in messages");
    }
}

#[test]
fn a_sealed_file_moves_without_ever_being_opened_and_only_opens_with_the_same_key() {
    let k = key(1);
    let (d, st) = protected("move", "alice", &k);
    st.seal(&k).ok();
    // open: refused; sealed: copied, byte for byte, as the owner alone may read it
    let (d2, st2) = (temp("move-b"), ());
    let _ = st2;
    let other = ProfileStore::new(&d2, "alice");
    let out = d.join("export.iykbp");
    st.open(&k).unwrap();
    assert_eq!(st.export_sealed(&out, "laptop", 1_000), Err(ProfileError::Open));
    st.seal(&k).unwrap();
    let n = st.export_sealed(&out, "laptop", 1_000).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(d.join("web-profile/alice.sealed")).unwrap());
    assert_eq!(n, std::fs::metadata(&out).unwrap().len());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(st.export_sealed(&out, "laptop", 1_001).is_err(), "never over an existing file");
    assert_assume_no_plaintext(&out);
    // the other machine: the wrong key, a damaged copy and a plain file are refused and change nothing
    assert_eq!(other.import_sealed(&out, &key(2), "desktop", 2_000), Err(ProfileError::WrongKey));
    let mut bad = std::fs::read(&out).unwrap();
    let last = bad.len() - 3;
    bad[last] ^= 0xFF;
    let badf = d.join("bad.iykbp");
    std::fs::write(&badf, bad).unwrap();
    assert!(matches!(other.import_sealed(&badf, &k, "desktop", 2_000), Err(ProfileError::Damaged(_))));
    std::fs::write(d.join("plain.txt"), "hello").unwrap();
    assert!(other.import_sealed(&d.join("plain.txt"), &k, "desktop", 2_000).is_err());
    assert_eq!(other.status(), Status::Off, "nothing was taken in");
    // the right key: taken in, opens with the key, and the files are the person's own
    other.import_sealed(&out, &k, "desktop", 2_000).unwrap();
    assert_eq!(other.status(), Status::Sealed);
    assert!(other.open(&k).unwrap() == false);
    assert_eq!(read(&other, "data/cookies.sqlite").contains(MARK), true);
    other.seal(&k).unwrap();
    // a second import keeps the first as the backup
    other.import_sealed(&out, &k, "desktop", 3_000).unwrap();
    assert!(d2.join("web-profile/alice.sealed.bak").exists());
}

fn assert_assume_no_plaintext(p: &Path) {
    let b = std::fs::read(p).unwrap();
    assert!(!b.windows(MARK.len()).any(|w| w == MARK.as_bytes()), "the moved file holds no readable cookie");
}

#[test]
fn one_session_holds_a_profile_at_a_time_and_a_claim_runs_out_or_is_given_back() {
    let k = key(1);
    let (d, st) = protected("lease", "alice", &k);
    assert_eq!(st.lease(100), None);
    st.take_lease("laptop", 100, 600).unwrap();
    assert_eq!(st.lease(150).unwrap().holder, "laptop");
    let e = st.take_lease("desktop", 150, 600).unwrap_err();
    assert!(e.contains("laptop") && e.contains("minute"), "{e}");
    st.take_lease("laptop", 200, 600).unwrap();
    // an export by another holder is refused while the claim stands; the sealed file is untouched
    assert!(st.export_sealed(&d.join("x.iykbp"), "desktop", 300).is_err());
    assert!(!d.join("x.iykbp").exists());
    // only the holder gives it back; others cannot
    st.release_lease("desktop", 300);
    assert!(st.lease(300).is_some());
    st.release_lease("laptop", 300);
    assert_eq!(st.lease(300), None);
    // it runs out by itself
    st.take_lease("laptop", 1_000, 600).unwrap();
    assert!(st.lease(1_599).is_some() && st.lease(1_600).is_none());
    st.take_lease("desktop", 1_700, 600).unwrap();
    assert!(st.take_lease("", 1_700, 60).is_err() && st.take_lease("bad\nname", 1_700, 60).is_err());
    // a move takes the claim for its own length and gives it back
    st.release_lease("desktop", 1_700);
    st.export_sealed(&d.join("y.iykbp"), "laptop", 2_000).unwrap();
    assert_eq!(st.lease(2_000), None);
    // refused while protection was never on, and while a plain profile stands beside a sealed one
    let off = ProfileStore::new(&temp("lease-off"), "bob");
    assert_eq!(off.export_sealed(&d.join("z.iykbp"), "laptop", 1), Err(ProfileError::NotProtected));
}

// ---- more checks of the move (F-VAULT-11, lane P-B), on the API above (export to a file, import from a file, one claim at a time)

#[test]
fn a_cut_or_wrongly_keyed_file_leaves_the_profile_here_byte_identical_and_no_half_written_file_behind() {
    let k = key(21);
    let (d, st) = protected("move-p1", "cy", &k);
    let before = std::fs::read(d.join("web-profile/cy.sealed")).unwrap();
    let (_d2, other) = protected("move-p2", "cy", &key(22));
    let theirs = d.join("theirs.iykbp");
    other.export_sealed(&theirs, "m", 1).unwrap();
    // sealed with another key
    assert_eq!(st.import_sealed(&theirs, &k, "m", 2), Err(ProfileError::WrongKey));
    // half of a good file
    let good = std::fs::read(&theirs).unwrap();
    let cut = d.join("cut.iykbp");
    std::fs::write(&cut, &good[..good.len() / 2]).unwrap();
    assert!(st.import_sealed(&cut, &key(22), "m", 2).is_err());
    assert_eq!(std::fs::read(d.join("web-profile/cy.sealed")).unwrap(), before, "the profile here is untouched");
    assert!(!d.join("web-profile/cy.sealed.new").exists(), "no half-written file");
    assert_eq!(st.lease(3), None, "a refused import gives its claim back");
    // a good import replaces it and the old file is the backup, byte for byte
    st.import_sealed(&theirs, &key(22), "m", 4).unwrap();
    assert_eq!(std::fs::read(d.join("web-profile/cy.sealed.bak")).unwrap(), before);
}

#[test]
fn an_import_waits_for_the_holder_of_the_claim_and_goes_through_once_the_claim_ran_out() {
    let k = key(23);
    let (d, st) = protected("move-p3", "di", &k);
    let (_d2, other) = protected("move-p4", "di", &k);
    let file = d.join("in.iykbp");
    other.export_sealed(&file, "m", 1).unwrap();
    st.take_lease("tenant-1", 100, 600).unwrap();
    let e = st.import_sealed(&file, &k, "machine-a", 200).unwrap_err();
    assert!(e.to_string().contains("tenant-1"), "the refusal says who has it: {e}");
    assert!(!d.join("web-profile/di.sealed.bak").exists(), "nothing changed");
    // after the claim of tenant-1 ran out the import goes through
    st.import_sealed(&file, &k, "machine-a", 1_000).unwrap();
    assert_eq!(st.lease(1_001), None);
}

#[test]
fn what_travels_holds_no_plain_content_and_opens_on_the_other_machine_with_the_same_key_after_the_first_one_closed_it() {
    let k = key(24);
    let (d, here) = protected("move-p5", "ed", &k);
    here.open(&k).unwrap();
    assert_eq!(here.export_sealed(&d.join("x.iykbp"), "m", 1), Err(ProfileError::Open), "nothing moves while it is open here");
    here.seal_for_exit(&k).unwrap();
    let out = d.join("x.iykbp");
    here.export_sealed(&out, "m", 2).unwrap();
    assert_assume_no_plaintext(&out);
    let d2 = temp("move-p6");
    let there = ProfileStore::with_run_root(&d2, "ed", &d2.join("run"));
    there.import_sealed(&out, &k, "tenant-1", 3).unwrap();
    there.open(&k).unwrap();
    assert!(read(&there, "data/cookies.sqlite").contains(MARK));
    there.seal_for_exit(&k).unwrap();
}

#[test]
fn a_running_session_holds_the_claim_a_second_machine_cannot_open_it_and_a_crashed_one_runs_out() {
    let k = key(1);
    let (d, st) = protected("claim", "alice", &k);
    let now = 1_000_000u64;
    // machine A signs in: opened, claim taken
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(1)), "pod-a", now), SignIn::Opened { .. }));
    assert_eq!(st.lease(now + 1).unwrap().holder, "pod-a");
    // machine B on the same volume: refused, and it says who has it; nothing is opened by B
    let live_before = st.status();
    match on_sign_in_as(&d, "alice", Some(key(1)), "pod-b", now + 5) {
        SignIn::Locked(m) => assert!(m.contains("pod-a") && m.contains("in use"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(st.status(), live_before);
    // A renews while open; the claim moves on and B is still refused near its end
    assert!(renew_claim(&d, "alice", "pod-a", now + 100));
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(1)), "pod-b", now + 200), SignIn::Locked(_)));
    // B cannot renew a claim that is not its own
    assert!(!renew_claim(&d, "alice", "pod-b", now + 110));
    // A signs out: sealed and the claim given back (through the holder name of this process)
    st.release_lease("pod-a", now + 120);
    on_sign_out(&d, "alice").unwrap();
    assert_eq!(st.status(), Status::Sealed);
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(1)), "pod-b", now + 130), SignIn::Opened { .. }), "after the claim was given back B may open");
    // B crashes: its claim runs out by itself and A may take over (the RAM copy of B is gone with its machine, here we seal it first)
    on_sign_out(&d, "alice").unwrap();
    st.take_lease("pod-b", now + 200, SESSION_CLAIM_SECS).unwrap();
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(1)), "pod-a", now + 200 + SESSION_CLAIM_SECS - 1), SignIn::Locked(_)));
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(1)), "pod-a", now + 200 + SESSION_CLAIM_SECS + 1), SignIn::Opened { .. }));
    // a wrong key gives the claim back so the next try is not blocked by the failed one
    on_sign_out(&d, "alice").ok();
    st.release_lease("pod-a", now + 1_000);
    assert!(matches!(on_sign_in_as(&d, "alice", Some(key(9)), "pod-a", now + 1_001), SignIn::Locked(_)));
    assert_eq!(st.lease(now + 1_002), None);
}

#[test]
fn the_holder_name_is_the_host_or_a_name_kept_once_beside_the_sealed_files() {
    let d = temp("holder");
    let a = holder_id(&d);
    assert!(!a.is_empty() && a.len() < 70);
    assert_eq!(holder_id(&d), a, "the same name every time");
    if std::env::var("HOSTNAME").map(|h| h.trim().is_empty()).unwrap_or(true) {
        assert!(a.starts_with("machine-") && d.join("web-profile/.holder").exists());
    }
}
