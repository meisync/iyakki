//! The boot counter of a system update (F-UPD-02) with a fake boot partition (a folder): the guard in the boot menu, the environment
//! block, arming after an update or rollback, marking good from the running system, and that nothing is armed without a fall-back.

use std::path::Path;

use super::bootcount::*;
use super::centre::*;
use super::tests_update::*;
use super::update::*;

fn key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

fn centre(r: &Rig) -> Centre {
    Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), r.release.clone(), false, Some(key().verifying_key())).with_data_free(500 << 20)
}

fn pack(r: &Rig, c: &Centre, version: &str) -> Found {
    let (dir, _) = payload(r, version, "macweb", false, 7);
    sign_pack(&dir, &key()).unwrap();
    read_pack(&dir, c.key()).unwrap()
}

fn menu(root: &Path) -> String {
    std::fs::read_to_string(find_ci(root, "boot/grub/grub.cfg").unwrap()).unwrap()
}

#[test]
fn the_environment_block_is_exactly_one_kilobyte_keeps_other_variables_and_reads_back() {
    let vars = vec![("saved_entry".to_string(), "0".to_string()), (VAR.to_string(), "1".to_string())];
    let b = format_env(&vars).unwrap();
    assert_eq!(b.len(), 1024);
    assert!(b.starts_with(b"# GRUB Environment Block\n") && b.ends_with(b"###"));
    assert_eq!(parse_env(&b), vars);
    let big = vec![("x".to_string(), "y".repeat(2000))];
    assert!(format_env(&big).is_none(), "too much for the block is refused, not cut");
    // a variable of GRUB's own stays when the counter changes
    let r = rig("bc-env");
    put(&r.boot.root, ENV_FILE, &format_env(&[("saved_entry".to_string(), "2".to_string())]).unwrap());
    put(&r.boot.root, "boot/grub/grub.cfg", with_previous_entry(&board_cfg(), "a", "0.1.0-alpha.0").unwrap().as_bytes());
    arm(&r.boot.root).unwrap();
    let got = parse_env(&std::fs::read(r.boot.root.join(ENV_FILE)).unwrap());
    assert_eq!(got, [("saved_entry".to_string(), "2".to_string()), (VAR.to_string(), "1".to_string())]);
    assert_eq!(read_state(&r.boot.root), State::Armed);
}

#[test]
fn the_guard_comes_with_the_previous_version_entry_before_the_first_entry_and_points_at_it() {
    let cfg = with_previous_entry(&board_cfg(), "0.1.0-alpha.1", "0.1.0-alpha.1").unwrap();
    assert!(has_guard(&cfg));
    let guard_at = cfg.find("iyakki boot counter: begin").unwrap();
    assert!(guard_at < cfg.find("menuentry \"IYAKKI\"").unwrap(), "before the first entry");
    assert!(guard_at > cfg.find("set default=").unwrap(), "after the default is set, so it can change it");
    // the normal entry is 0, the previous version 1: that is what the fall-back selects
    assert!(cfg.contains("set default=1\n"), "{cfg}");
    let top: Vec<&str> = cfg.lines().filter(|l| l.starts_with("menuentry ")).collect();
    assert!(top[0].contains("\"IYAKKI\"") && top[1].contains(PREV_TITLE));
    // a second pass replaces the guard, it does not stack another one
    let again = with_previous_entry(&cfg, "0.1.0-alpha.2", "0.1.0-alpha.2").unwrap();
    assert_eq!(again.matches("iyakki boot counter: begin").count(), 1);
    // without the previous entry there is nothing to fall back to, so no guard is made
    assert!(with_guard(&board_cfg()).unwrap_err().contains("no previous-version entry"));
    assert_eq!(without_guard(&cfg).matches("load_env").count(), 0);
    assert_eq!(without_guard(&cfg), with_previous_entry(&board_cfg(), "0.1.0-alpha.1", "0.1.0-alpha.1").map(|t| without_guard(&t)).unwrap());
}

#[test]
fn an_update_arms_the_counter_and_the_first_good_picture_marks_it_good() {
    let r = rig("bc-update");
    let c = centre(&r);
    assert_eq!(read_state(&r.boot.root), State::Idle);
    let msg = c.install(&pack(&r, &c, "0.1.0-alpha.2"), false, "admin").unwrap();
    assert!(msg.contains("starts the saved version by itself"), "{msg}");
    assert_eq!(read_state(&r.boot.root), State::Armed);
    assert!(has_guard(&menu(&r.boot.root)) && menu(&r.boot.root).contains(PREV_TITLE));
    // this is still the start in which the update was written: nothing is marked
    let t = read_trial(r.dm.data_dir()).unwrap();
    assert!(c.confirm_boot(&t.boot_id).is_none() || t.boot_id.is_empty());
    assert_eq!(read_state(&r.boot.root), State::Armed);
    // the next start runs the new version: marked good, and a second call has nothing left to do
    let rel = std::fs::read_to_string(r.boot.root.join("boot/iyakki-release")).unwrap();
    let running = Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), rel, false, None);
    let line = running.confirm_boot("another-boot").unwrap();
    assert!(line.contains("0.1.0-alpha.2") && line.contains("marked good"), "{line}");
    assert_eq!(read_state(&r.boot.root), State::Idle);
    assert!(running.confirm_boot("another-boot").is_none());
}

#[test]
fn a_start_that_still_runs_the_old_version_is_not_marked_good() {
    let r = rig("bc-old");
    let c = centre(&r);
    c.install(&pack(&r, &c, "0.1.0-alpha.2"), false, "admin").unwrap();
    // the release of the old version is what runs: the new one did not start
    assert!(c.confirm_boot("another-boot").is_none());
    assert_eq!(read_state(&r.boot.root), State::Armed, "the guard of the menu decides what happens next");
    // a RAM-only data volume is never touched
    let volatile = Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), r.release.clone(), true, None);
    assert!(volatile.confirm_boot("another-boot").is_none());
}

#[test]
fn nothing_is_armed_when_the_menu_cannot_fall_back() {
    let r = rig("bc-none");
    // a menu without the previous-version entry and guard (an image from before the update was ever used)
    assert!(arm(&r.boot.root).unwrap_err().contains("would not fall back"));
    assert_eq!(read_state(&r.boot.root), State::Idle);
    assert!(!r.boot.root.join(ENV_FILE).exists());
    // a guard without an entry to go to is refused too
    let broken = board_cfg().replace("menuentry \"IYAKKI\" {", "# iyakki boot counter: begin\n# iyakki boot counter: end\nmenuentry \"IYAKKI\" {");
    put(&r.boot.root, "boot/grub/grub.cfg", broken.as_bytes());
    assert!(arm(&r.boot.root).is_err());
    // no menu at all
    std::fs::remove_file(r.boot.root.join("boot/grub/grub.cfg")).unwrap();
    assert!(arm(&r.boot.root).unwrap_err().contains("missing"));
}

#[test]
fn a_rollback_arms_the_counter_too() {
    let r = rig("bc-roll");
    let c = centre(&r);
    c.install(&pack(&r, &c, "0.1.0-alpha.2"), false, "admin").unwrap();
    assert!(mark_good(&r.boot.root).unwrap(), "the new version was good");
    assert_eq!(read_state(&r.boot.root), State::Idle);
    let msg = c.rollback("admin").unwrap();
    assert!(msg.contains("starts the saved version by itself"), "{msg}");
    assert_eq!(read_state(&r.boot.root), State::Armed);
    assert!(!mark_good(&r.boot.root).is_err());
}
