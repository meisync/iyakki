use super::migrate::*;
use super::tests::*;
use super::*;
use std::os::unix::fs::PermissionsExt;

fn populate(h: &Path) {
    std::fs::create_dir_all(h.join("Documents/deep/er")).unwrap();
    std::fs::create_dir_all(h.join(".config")).unwrap();
    std::fs::write(h.join("Documents/a.txt"), b"alpha").unwrap();
    std::fs::write(h.join("Documents/deep/er/b.bin"), vec![7u8; 300_000]).unwrap();
    std::fs::write(h.join("Documents/empty"), b"").unwrap();
    std::fs::write(h.join(".config/ünï-ço.txt"), "unicode name").unwrap();
    std::fs::write(h.join("run.sh"), b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(h.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(h.join(".config"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink("Documents/a.txt", h.join("link")).unwrap();
}

fn same(a: &Path, b: &Path) {
    assert_eq!(verify(a, b).unwrap(), 5);
}

#[test]
fn the_plan_counts_what_will_be_copied_and_changes_nothing() {
    let r = rig("plan");
    let h = r.home("amy");
    populate(&h);
    let _sock = std::os::unix::net::UnixListener::bind(h.join("agent.sock")).unwrap();
    let p = plan(&r.env, "amy").unwrap();
    assert_eq!((p.files, p.dirs, p.links), (5, 4, 1));
    assert_eq!(p.bytes, 5 + 300_000 + 12 + 10);
    assert_eq!(p.skipped, vec!["agent.sock"]);
    assert!(p.ok() && p.payload >= MIN_PAYLOAD && p.payload % 4096 == 0);
    let d = p.describe();
    assert!(d.contains("COPIED") && d.contains("SHA-256") && d.contains("does not erase its old blocks") && d.contains("agent.sock"), "{d}");
    assert!(!r.env.exists("amy") && r.kernel.log().is_empty());
}

#[test]
fn the_plan_reports_problems_in_words() {
    let r = rig("planbad");
    let h = r.home("amy");
    populate(&h);
    let small = EncHome::with(&r.env.data_dir, &r.env.home_parent, r.kernel.clone(), SMALL_KDF, Box::new(Seq(5)), Box::new(|_| Some(100 << 20)), Box::new(|| 0));
    let p = plan(&small, "amy").unwrap();
    assert!(!p.ok() && p.problems[0].contains("not enough free space"), "{:?}", p.problems);
    assert!(matches!(run(&small, "amy", "pw", &mut |_| true), Err(HomeError::NotConfirmed(_))));
    std::fs::create_dir_all(r.env.plain_copy_path("amy")).unwrap();
    assert!(plan(&r.env, "amy").unwrap().problems.iter().any(|p| p.contains("already exists")));
    assert_eq!(plan(&r.env, "../x").unwrap_err(), HomeError::BadAccount);
}

const SMALL_KDF: KdfParams = SMALL;

#[test]
fn a_migration_copies_verifies_switches_and_keeps_the_old_home() {
    let r = rig("migrate");
    let h = r.home("amy");
    populate(&h);
    let mut seen = vec![];
    let rep = run(&r.env, "amy", "amy-pass-1", &mut |s| {
        seen.push(s);
        true
    })
    .unwrap();
    assert_eq!(seen, vec![Step::Planned, Step::ContainerMade, Step::StagingMounted, Step::Copied, Step::Verified, Step::StagingUnmounted, Step::PlainRenamed, Step::Switched]);
    assert_eq!(rep.files, 5);
    assert_eq!(rep.plain_copy, r.env.plain_copy_path("amy"));
    assert!(!rep.recovery.0.is_empty());
    // the home is now the encrypted one (mounted), with every file; the old one is beside it, unchanged
    assert_eq!(r.env.status("amy").state, State::Open);
    same(&r.env.plain_copy_path("amy"), &h);
    assert_eq!(std::fs::read_link(h.join("link")).unwrap(), Path::new("Documents/a.txt"));
    assert_eq!(std::fs::metadata(h.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::metadata(h.join(".config")).unwrap().permissions().mode() & 0o777, 0o700);
    assert!(r.env.status("amy").plain_copy.is_some());
    assert!(!r.env.dir().join("amy.building").exists() && !r.env.dir().join("amy.staging").exists());
    // locked: the files are not visible at the home
    r.env.lock("amy").unwrap();
    assert!(!h.join("Documents/a.txt").exists());
    r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap();
    assert_eq!(std::fs::read(h.join("Documents/a.txt")).unwrap(), b"alpha");
}

#[test]
fn a_copy_that_differs_is_caught_and_nothing_is_switched() {
    // corruption of the copy after it was written (a bad block), and a change to the original during the copy
    for what in ["copy", "original", "extra"] {
        let r = rig("corrupt");
        let h = r.home("amy");
        populate(&h);
        let staging = r.env.dir().join("amy.staging");
        let err = run(&r.env, "amy", "amy-pass-1", &mut |s| {
            if s == Step::Copied {
                match what {
                    "copy" => std::fs::write(staging.join("Documents/a.txt"), b"alphA").unwrap(),
                    "original" => std::fs::write(h.join("Documents/a.txt"), b"alphb").unwrap(),
                    _ => std::fs::write(staging.join("surprise"), b"x").unwrap(),
                }
            }
            true
        })
        .unwrap_err();
        assert!(err.to_string().contains("verification failed"), "{what}: {err}");
        assert!(!r.env.exists("amy") && !r.env.meta_path("amy").exists(), "{what}: container left");
        assert!(!r.env.plain_copy_path("amy").exists(), "{what}: switched anyway");
        assert!(h.join("Documents/a.txt").exists(), "{what}: the plain home must be untouched");
        assert!(r.kernel.mapping_names().is_empty() && r.kernel.loop_devices().is_empty());
        assert!(!r.env.dir().join("amy.building").exists());
    }
}

#[test]
fn a_kernel_failure_mid_way_undoes_everything() {
    for op in ["mkfs", "mount"] {
        let r = rig("kfail");
        let h = r.home("amy");
        populate(&h);
        r.kernel.fail_once(op);
        assert!(run(&r.env, "amy", "amy-pass-1", &mut |_| true).is_err());
        assert!(!r.env.exists("amy") && h.join("Documents/a.txt").exists() && !r.env.plain_copy_path("amy").exists(), "{op}");
        assert!(r.kernel.mapping_names().is_empty());
    }
    // the final mount failing after the old home was renamed: the old home is put back
    let r = rig("kfail2");
    let h = r.home("amy");
    populate(&h);
    let k = r.kernel.clone();
    let _ = run(&r.env, "amy", "amy-pass-1", &mut |s| {
        if s == Step::PlainRenamed {
            k.fail_once("mount");
        }
        true
    })
    .unwrap_err();
    assert!(h.join("Documents/a.txt").exists() && !r.env.plain_copy_path("amy").exists() && !r.env.exists("amy"));
}

#[test]
fn a_power_cut_at_every_step_loses_nothing_and_recovery_settles_it() {
    let steps = [Step::Planned, Step::ContainerMade, Step::StagingMounted, Step::Copied, Step::Verified, Step::StagingUnmounted, Step::PlainRenamed];
    for cut in steps {
        let r = rig("cut");
        let h = r.home("amy");
        populate(&h);
        let _ = run(&r.env, "amy", "amy-pass-1", &mut |s| s != cut);
        r.kernel.power_cut();
        let before_rename = matches!(cut, Step::Planned | Step::ContainerMade | Step::StagingMounted | Step::Copied | Step::Verified | Step::StagingUnmounted);
        // the data is whole in one of two places at every moment
        let original = if before_rename { h.clone() } else { r.env.plain_copy_path("amy") };
        same(&original, &original);
        assert_eq!(std::fs::read(original.join("Documents/a.txt")).unwrap(), b"alpha", "{cut:?}");
        let said = recover_interrupted(&r.env, "amy");
        if cut == Step::Planned {
            assert!(said.is_none(), "nothing was started");
            continue;
        }
        let said = said.unwrap_or_else(|| panic!("{cut:?}: nothing said"));
        assert!(!r.env.dir().join("amy.building").exists(), "{cut:?}");
        if before_rename {
            assert!(said.contains("undone"), "{cut:?}: {said}");
            assert!(!r.env.exists("amy") && h.join("Documents/a.txt").exists(), "{cut:?}");
        } else {
            assert!(said.contains("completed"), "{cut:?}: {said}");
            assert!(r.env.exists("amy"));
            // the next sign-in mounts the container: every file is there
            assert_eq!(r.env.on_login("amy", "amy-pass-1"), LoginHome::Opened, "{cut:?}");
            assert_eq!(std::fs::read(h.join("Documents/a.txt")).unwrap(), b"alpha");
            assert!(r.env.plain_copy_path("amy").is_dir());
        }
    }
}

#[test]
fn the_plain_copy_is_removed_only_after_the_person_confirms_and_the_flash_note_is_said() {
    let r = rig("removeplain");
    let h = r.home("amy");
    populate(&h);
    let rep = run(&r.env, "amy", "amy-pass-1", &mut |_| true).unwrap();
    let plain = r.env.plain_copy_path("amy");
    // the recovery key must be typed back first
    assert!(matches!(remove_plain_copy(&r.env, "amy"), Err(HomeError::NotConfirmed(_))));
    assert!(plain.is_dir());
    r.env.confirm_recovery("amy", &rep.recovery.0).unwrap();
    r.env.lock("amy").unwrap();
    assert_eq!(remove_plain_copy(&r.env, "amy"), Err(HomeError::NotOpen), "the encrypted home must be open and checked first");
    r.env.unlock("amy", &Secret::password("amy-pass-1")).unwrap();
    let said = remove_plain_copy(&r.env, "amy").unwrap();
    assert!(!plain.exists() && said.contains("does not erase its old blocks"));
    assert!(matches!(remove_plain_copy(&r.env, "amy"), Err(HomeError::NotConfirmed(_))));
    assert_eq!(std::fs::read(h.join("Documents/a.txt")).unwrap(), b"alpha");
}

#[test]
fn nothing_migrates_by_itself() {
    let r = rig("optin");
    let h = r.home("amy");
    populate(&h);
    assert_eq!(r.env.on_login("amy", "amy-pass-1"), LoginHome::None);
    assert!(r.env.on_logout("amy").is_none() && r.env.on_screen_lock("amy").is_none() && r.env.on_password_change("amy", "a", "b").is_none());
    assert!(r.kernel.log().is_empty(), "no kernel operation without a request");
    assert!(!r.env.dir().exists() || std::fs::read_dir(r.env.dir()).unwrap().next().is_none());
    same(&h, &h);
}

#[test]
fn verify_notices_mode_link_and_extra_entry_differences() {
    let r = rig("verify");
    let (a, b) = (r.root.join("a"), r.root.join("b"));
    for d in [&a, &b] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("f"), b"same").unwrap();
        std::os::unix::fs::symlink("f", d.join("l")).unwrap();
    }
    assert_eq!(verify(&a, &b).unwrap(), 1);
    std::fs::set_permissions(b.join("f"), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(a.join("f"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(verify(&a, &b).is_err());
    std::fs::set_permissions(b.join("f"), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(b.join("l")).unwrap();
    std::os::unix::fs::symlink("elsewhere", b.join("l")).unwrap();
    assert!(verify(&a, &b).is_err());
}
