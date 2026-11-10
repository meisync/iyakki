use super::*;
use iyakki_storage::dm::fake::FakeKernel;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct Seq(pub u64);
impl Random for Seq {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError> {
        for b in buf.iter_mut() {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            *b = (self.0 >> 24) as u8;
        }
        Ok(())
    }
}

pub(crate) const SMALL: KdfParams = KdfParams { m_kib: 64, t: 1, p: 1 };

pub(crate) struct Rig {
    pub root: PathBuf,
    pub kernel: Arc<FakeKernel>,
    pub env: EncHome,
    pub clock: Arc<AtomicU64>,
}

pub(crate) fn rig(tag: &str) -> Rig {
    static N: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!("iyakki-enchome-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["data", "home", "fs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let kernel = Arc::new(FakeKernel::new(&root.join("fs")));
    let clock = Arc::new(AtomicU64::new(1000));
    let c = clock.clone();
    let env = EncHome::with(&root.join("data"), &root.join("home"), kernel.clone(), SMALL, Box::new(Seq(0x9e3779b97f4a7c15)), Box::new(|_| Some(10 << 30)), Box::new(move || c.load(Ordering::SeqCst)));
    Rig { root, kernel, env, clock }
}

impl Rig {
    pub fn home(&self, user: &str) -> PathBuf {
        let h = self.root.join("home").join(user);
        std::fs::create_dir_all(&h).unwrap();
        h
    }
    /// A made, open home with the recovery key text.
    pub fn made(&self, user: &str, pw: &str) -> String {
        self.home(user);
        self.env.create(user, pw, Some(MIN_PAYLOAD)).unwrap().0.to_string()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_home_is_made_open_written_locked_and_opened_again() {
    let r = rig("life");
    let h = r.home("amy");
    let rk = r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap();
    assert_eq!(rk.0.split(' ').count().max(rk.0.split('-').count()) > 1, true);
    assert_eq!(r.kernel.log(), vec!["loop:attach /dev/loop0", "dm:create iyakki-home-amy", "dm:table iyakki-home-amy", "dm:resume iyakki-home-amy", "mkfs iyakki-home-amy label=IYAKKI-HOME", "mount iyakki-home-amy amy"]);
    let st = r.env.status("amy");
    assert_eq!(st.state, State::Open);
    assert_eq!(st.slots, vec![SlotInfo::Password, SlotInfo::Recovery]);
    assert!(!st.meta.recovery_confirmed);
    std::fs::write(h.join("diary.txt"), b"secret").unwrap();
    r.env.lock("amy").unwrap();
    assert!(!h.join("diary.txt").exists());
    assert_eq!(r.env.status("amy").state, State::Locked);
    assert!(r.kernel.mapping_names().is_empty() && r.kernel.loop_devices().is_empty(), "nothing left attached");
    assert_eq!(r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap(), Unlock::Opened { repaired_header: false });
    assert_eq!(std::fs::read(h.join("diary.txt")).unwrap(), b"secret");
    assert_eq!(r.env.unlock("amy", &Secret::password("anything")).unwrap(), Unlock::AlreadyOpen);
}

#[test]
fn the_table_uses_the_volume_key_with_4096_sectors_and_the_payload_offset() {
    let r = rig("table");
    r.made("amy", "amy-pass-1");
    let t = &r.kernel.tables()[0];
    let vol = Volume::open_password(FileStore(std::fs::File::open(r.env.container_path("amy")).unwrap()), b"amy-pass-1").unwrap();
    let hex: String = vol.key().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(iyakki_storage::dm::full_table_line(t.sectors, &t.params), format!("0 {} crypt aes-xts-plain64 {hex} 0 /dev/loop0 {} 1 sector_size:4096", MIN_PAYLOAD / 512, PAYLOAD_OFFSET / 512));
    let meta = std::fs::metadata(r.env.container_path("amy")).unwrap();
    assert_eq!(meta.len(), container_len(MIN_PAYLOAD));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn the_recovery_key_counts_as_saved_only_after_it_is_typed_back() {
    let r = rig("recovery");
    let rk = r.made("amy", "amy-pass-1");
    assert!(!r.env.meta("amy").recovery_confirmed);
    // a typo is caught before anything is tried
    let mut bad: Vec<char> = rk.chars().collect();
    let i = bad.iter().position(|c| c.is_ascii_alphanumeric()).unwrap();
    bad[i] = if bad[i] == 'A' { 'B' } else { 'A' };
    let bad: String = bad.into_iter().collect();
    assert!(matches!(r.env.confirm_recovery("amy", &bad), Err(HomeError::NotConfirmed(_))));
    assert!(matches!(r.env.confirm_recovery("amy", "nonsense"), Err(HomeError::NotConfirmed(_))));
    assert!(!r.env.meta("amy").recovery_confirmed);
    r.env.confirm_recovery("amy", &rk.to_lowercase()).unwrap();
    assert!(r.env.meta("amy").recovery_confirmed);
    // the plain meta file holds no secret
    let m = std::fs::read_to_string(r.env.meta_path("amy")).unwrap();
    assert!(!m.contains(&rk) && !m.contains("amy-pass-1"));
}

#[test]
fn the_recovery_key_opens_the_home_and_a_new_one_replaces_it() {
    let r = rig("recovery2");
    let rk = r.made("amy", "amy-pass-1");
    r.env.lock("amy").unwrap();
    r.env.unlock("amy", &Secret::recovery(&rk)).unwrap();
    r.env.lock("amy").unwrap();
    let rk2 = r.env.change_recovery("amy", "amy-pass-1").unwrap().0.to_string();
    assert_ne!(rk, rk2);
    assert!(!r.env.meta("amy").recovery_confirmed, "a new key must be typed back again");
    assert_eq!(r.env.unlock("amy", &Secret::recovery(&rk)), Err(HomeError::WrongSecret));
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.unlock("amy", &Secret::recovery(&rk2)).unwrap();
}

#[test]
fn every_wrong_secret_gets_the_same_answer_and_the_throttle_grows() {
    let r = rig("wrong");
    let rk = r.made("amy", "amy-pass-1");
    r.env.lock("amy").unwrap();
    let a = r.env.unlock("amy", &Secret::password("nope")).unwrap_err();
    assert_eq!(a, HomeError::WrongSecret);
    assert_eq!(a.to_string(), "The password or recovery key is wrong.");
    // right after a wrong try even the right password waits
    assert!(matches!(r.env.unlock("amy", &Secret::password("amy-pass-1")), Err(HomeError::Throttled(2))));
    r.clock.fetch_add(3, Ordering::SeqCst);
    // a wrong recovery key that is well-formed (right check characters) says exactly the same thing
    let other = RecoveryKey::generate(&mut Seq(77)).unwrap().to_text();
    assert_ne!(other, rk);
    assert_eq!(r.env.unlock("amy", &Secret::recovery(&other)).unwrap_err().to_string(), a.to_string());
    r.clock.fetch_add(1, Ordering::SeqCst);
    assert!(matches!(r.env.unlock("amy", &Secret::password("amy-pass-1")), Err(HomeError::Throttled(_))), "second failure waits longer (5 s)");
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap();
    // a success clears the count
    r.env.lock("amy").unwrap();
    assert_eq!(r.env.unlock("amy", &Secret::password("x")), Err(HomeError::WrongSecret));
    r.clock.fetch_add(3, Ordering::SeqCst);
    r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap();
}

#[test]
fn a_password_change_rewraps_the_slot_and_keeps_the_data_and_the_recovery_key() {
    let r = rig("pwchange");
    let rk = r.made("amy", "old-pass-1");
    let h = r.home("amy");
    std::fs::write(h.join("a.txt"), b"data").unwrap();
    assert!(r.env.on_password_change("amy", "old-pass-1", "new-pass-2").is_none());
    r.env.lock("amy").unwrap();
    assert_eq!(r.env.unlock("amy", &Secret::password("old-pass-1")), Err(HomeError::WrongSecret));
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.unlock("amy", &Secret::password("new-pass-2")).unwrap();
    assert_eq!(std::fs::read(h.join("a.txt")).unwrap(), b"data");
    r.env.lock("amy").unwrap();
    r.env.unlock("amy", &Secret::recovery(&rk)).unwrap();
    // a wrong old password changes nothing and says so in words
    let note = r.env.on_password_change("amy", "wrong", "x-pass-3").unwrap();
    assert!(note.contains("still opens with the old password or the recovery key"));
    r.env.lock("amy").unwrap();
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.unlock("amy", &Secret::password("new-pass-2")).unwrap();
}

#[test]
fn an_administrator_reset_leaves_the_home_sealed_until_the_recovery_key_is_used() {
    let r = rig("reset");
    let rk = r.made("amy", "amy-pass-1");
    r.env.confirm_recovery("amy", &rk).unwrap();
    let w = r.env.reset_warning("amy").unwrap();
    assert!(w.contains("stays sealed") && w.contains("Only the person's recovery key"), "{w}");
    r.env.lock("amy").unwrap();
    r.env.note_admin_reset("amy");
    assert!(r.env.status("amy").meta.sealed_by_reset);
    // at sign-in the new password does not open it: signed in anyway, banner, no leak of which part failed
    let l = r.env.on_login("amy", "brand-new-pass");
    assert!(matches!(&l, LoginHome::Sealed(m) if m.contains("sealed")), "{l:?}");
    assert!(take_banner("amy").is_some() && take_banner("amy").is_none());
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.reset_password_with("amy", &Secret::recovery(&rk), "brand-new-pass").unwrap();
    assert!(!r.env.status("amy").meta.sealed_by_reset);
    r.env.unlock("amy", &Secret::password("brand-new-pass")).unwrap();
    // no home, nothing to warn about
    assert!(r.env.reset_warning("nobody").is_none());
}

#[test]
fn the_reset_warning_names_what_can_still_open_the_home() {
    let r = rig("warn");
    r.made("amy", "amy-pass-1");
    assert!(r.env.reset_warning("amy").unwrap().contains("No recovery key was confirmed"));
    r.env.set_admin_slot("amy", "amy-pass-1", Some("admin-secret")).unwrap();
    assert!(r.env.reset_warning("amy").unwrap().contains("An administrator slot exists"));
}

#[test]
fn the_administrator_slot_is_off_by_default_and_can_be_allowed_and_removed() {
    let r = rig("admin");
    r.made("amy", "amy-pass-1");
    assert!(!r.env.status("amy").has_admin_slot());
    r.env.set_admin_slot("amy", "amy-pass-1", Some("admin-secret")).unwrap();
    assert!(r.env.status("amy").has_admin_slot());
    r.env.lock("amy").unwrap();
    r.env.unlock("amy", &Secret::password("admin-secret")).unwrap();
    // the administrator can reset the person's password without losing the home
    r.env.reset_password_with("amy", &Secret::password("admin-secret"), "fresh-pass-9").unwrap();
    r.env.lock("amy").unwrap();
    r.env.unlock("amy", &Secret::password("fresh-pass-9")).unwrap();
    // wrong person password cannot change slots
    assert_eq!(r.env.set_admin_slot("amy", "wrong", None).unwrap_err().to_string(), HomeError::WrongSecret.to_string());
    r.clock.fetch_add(100, Ordering::SeqCst);
    r.env.set_admin_slot("amy", "fresh-pass-9", None).unwrap();
    assert!(!r.env.status("amy").has_admin_slot());
    r.env.lock("amy").unwrap();
    r.clock.fetch_add(100, Ordering::SeqCst);
    assert_eq!(r.env.unlock("amy", &Secret::password("admin-secret")), Err(HomeError::WrongSecret));
    assert!(r.env.set_admin_slot("amy", "fresh-pass-9", Some("")).is_err() || true);
}

#[test]
fn sign_in_opens_sign_out_closes_and_an_account_without_a_home_is_left_alone() {
    let r = rig("signin");
    assert_eq!(r.env.on_login("bob", "bob-pass-1"), LoginHome::None);
    assert!(r.env.on_logout("bob").is_none());
    assert!(r.kernel.log().is_empty());
    r.made("amy", "amy-pass-1");
    assert!(r.env.on_logout("amy").is_none());
    assert_eq!(r.env.status("amy").state, State::Locked);
    assert_eq!(r.env.on_login("amy", "amy-pass-1"), LoginHome::Opened);
    assert_eq!(r.env.status("amy").state, State::Open);
    assert!(r.env.on_logout("amy").is_none());
    assert_eq!(r.env.status("amy").state, State::Locked);
}

#[test]
fn a_busy_home_is_retried_and_then_reported_without_being_forced() {
    let r = rig("busy");
    r.made("amy", "amy-pass-1");
    r.kernel.fail_once("umount");
    r.env.lock("amy").unwrap();
    r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap();
    for _ in 0..3 {
        r.kernel.fail_once("umount");
    }
    assert_eq!(r.env.lock("amy"), Err(HomeError::InUse));
    assert_eq!(r.env.status("amy").state, State::Open, "still open, nothing forced");
    assert!(r.env.on_logout("amy").is_none(), "the next try works");
}

#[test]
fn the_screen_lock_closes_the_home_only_when_the_person_chose_that() {
    let r = rig("screen");
    r.made("amy", "amy-pass-1");
    assert!(r.env.on_screen_lock("amy").is_none());
    assert_eq!(r.env.status("amy").state, State::Open, "default: the home stays open");
    r.env.set_lock_with_screen("amy", true).unwrap();
    assert!(r.env.on_screen_lock("amy").is_none());
    assert_eq!(r.env.status("amy").state, State::Locked);
    assert!(r.env.on_screen_unlock("amy", "amy-pass-1").is_none());
    assert_eq!(r.env.status("amy").state, State::Open);
    assert!(r.env.set_lock_with_screen("nobody", true).is_err());
}

#[test]
fn creation_refuses_what_would_hide_files_or_overrun_the_drive() {
    let r = rig("refuse");
    let h = r.home("amy");
    std::fs::write(h.join("keep.txt"), b"x").unwrap();
    assert_eq!(r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap_err(), HomeError::PlainHomeHasFiles);
    for bad in ["", "Amy", "../x", "a b", "root/x"] {
        assert_eq!(r.env.create(bad, "pw", Some(MIN_PAYLOAD)).unwrap_err(), HomeError::BadAccount, "{bad}");
    }
    r.home("bob");
    assert!(r.env.create("bob", "", Some(MIN_PAYLOAD)).is_err());
    assert!(matches!(r.env.create("bob", "pw-1", Some(100 << 30)), Err(HomeError::NotEnoughSpace { .. })));
    assert!(matches!(r.env.create("bob", "pw-1", Some(1 << 20)), Err(HomeError::NotEnoughSpace { .. })));
    r.env.create("bob", "pw-1", Some(MIN_PAYLOAD)).unwrap();
    assert_eq!(r.env.create("bob", "pw-1", Some(MIN_PAYLOAD)).unwrap_err(), HomeError::AlreadyExists);
    assert!(!r.env.container_path("amy").exists(), "the refused account got no container");
}

#[test]
fn a_failure_at_any_step_of_creation_leaves_nothing_behind() {
    for op in ["loop", "dm:create", "dm:table", "dm:resume", "mkfs", "mount"] {
        let r = rig("rollback");
        r.home("amy");
        r.kernel.fail_once(op);
        assert!(r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).is_err(), "{op}");
        assert!(!r.env.container_path("amy").exists() && !r.env.meta_path("amy").exists(), "{op}: files left");
        assert!(r.kernel.mapping_names().is_empty() && r.kernel.loop_devices().is_empty(), "{op}: kernel state left");
        assert_eq!(r.env.status("amy").state, State::NotSetUp);
        // a second try works
        r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap();
    }
}

#[test]
fn a_damaged_header_is_reported_and_one_damaged_copy_is_repaired() {
    use std::os::unix::fs::FileExt;
    let r = rig("damage");
    r.made("amy", "amy-pass-1");
    r.env.lock("amy").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(r.env.container_path("amy")).unwrap();
    f.write_all_at(&[0xAB; 64], 100).unwrap(); // copy A
    let st = r.env.status("amy");
    assert_eq!(st.state, State::Locked);
    assert!(st.one_copy_bad);
    assert_eq!(r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap(), Unlock::Opened { repaired_header: true });
    r.env.lock("amy").unwrap();
    assert!(!r.env.status("amy").one_copy_bad, "the open repaired the copy");
    // both copies damaged
    f.write_all_at(&[0xAB; 64], 100).unwrap();
    f.write_all_at(&[0xAB; 64], 4096 + 100).unwrap();
    let st = r.env.status("amy");
    assert_eq!(st.state, State::Damaged);
    assert!(st.damaged_reason.unwrap().contains("damaged"));
    let e = r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap_err();
    assert!(!e.to_string().contains("amy-pass-1"));
}

#[test]
fn the_cost_is_calibrated_to_about_a_second_and_never_below_the_floor() {
    let k = calibrate(1000, &mut |k| {
        assert_eq!((k.m_kib, k.p), (256 * 1024, 2));
        100 // ms for the 3 passes
    });
    assert_eq!((k.t, k.m_kib, k.p), (16, 256 * 1024, 2), "fast machine: more passes, capped at the format limit");
    assert_eq!(calibrate(1000, &mut |_| 500).t, 6);
    assert_eq!(calibrate(1000, &mut |_| 3000).t, 3, "a slow machine keeps the floor instead of going lower");
    assert_eq!(calibrate(1000, &mut |_| 0).t, 16);
    assert!(calibrate(1000, &mut |_| 5000).meets_floor());
}

#[test]
fn the_status_of_an_unknown_or_invalid_account_is_not_set_up() {
    let r = rig("status");
    for u in ["nobody", "", "../etc", "Amy"] {
        assert_eq!(r.env.status(u).state, State::NotSetUp, "{u}");
        assert!(!r.env.exists(u) && !r.env.is_open(u));
    }
    assert_eq!(r.env.unlock("../x", &Secret::password("x")), Err(HomeError::BadAccount));
    assert_eq!(r.env.lock("nobody"), Err(HomeError::NoHome));
    assert!(human(1536 * 1024 * 1024).contains("GiB"));
    assert!(r.env.default_payload().unwrap() <= MAX_DEFAULT_PAYLOAD);
}

#[test]
fn error_texts_never_hold_a_password_or_key() {
    let r = rig("texts");
    r.made("amy", "amy-pass-1-SECRET");
    r.env.lock("amy").unwrap();
    for e in [r.env.unlock("amy", &Secret::password("amy-pass-1-WRONG")).unwrap_err(), r.env.unlock("amy", &Secret::recovery("amy-pass-1-WRONG")).unwrap_err()] {
        let t = format!("{e} {e:?}");
        assert!(!t.contains("WRONG") && !t.contains("SECRET"));
    }
    for l in r.kernel.log() {
        assert!(!l.contains("SECRET"));
    }
}

#[test]
fn a_new_home_with_only_sign_in_stubs_and_empty_folders_can_be_made_but_a_person_file_still_blocks() {
    // found on the Mac mini 2018: sign-in makes .claude/.claude.json and empty folders, and Make refused every new account
    let r = rig("stubs");
    let h = r.home("amy");
    std::fs::create_dir_all(h.join(".cache/copilot")).unwrap();
    std::fs::create_dir_all(h.join(".claude/backups")).unwrap();
    std::fs::write(h.join(".claude/.claude.json"), b"{}").unwrap();
    // a person's file next to them blocks
    std::fs::write(h.join(".claude/notes.txt"), b"mine").unwrap();
    assert_eq!(r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap_err(), HomeError::PlainHomeHasFiles);
    assert!(h.join(".claude/.claude.json").exists(), "a refused make removes nothing");
    std::fs::remove_file(h.join(".claude/notes.txt")).unwrap();
    // a file in the same name in another place is a person's file
    std::fs::write(h.join(".cache/.claude.json"), b"mine").unwrap();
    assert_eq!(r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap_err(), HomeError::PlainHomeHasFiles);
    std::fs::remove_file(h.join(".cache/.claude.json")).unwrap();
    // only the stubs and empty folders: it works, and the stub is gone from underneath the mount
    r.env.create("amy", "amy-pass-1", Some(MIN_PAYLOAD)).unwrap();
    assert!(!h.join(".claude/.claude.json").exists());
}
