//! The Update centre engine with fake packs and a fake boot partition (a folder with a size limit): finding packs, signatures, the
//! preview and what needs a yes, install, damaged payloads, rollback and rolling forward, the check after a start, and that nothing
//! outside the boot files and the engine's own data folders is ever written.

use std::path::{Path, PathBuf};

use super::centre::*;
use super::tests_update::*;
use super::update::*;

fn key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

fn centre(r: &Rig) -> Centre {
    centre_with(r, Some(key().verifying_key()))
}

fn centre_with(r: &Rig, k: Option<ed25519_dalek::VerifyingKey>) -> Centre {
    Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), r.release.clone(), false, k).with_data_free(500 << 20)
}

fn found(r: &Rig, c: &Centre, version: &str, variant: &str, sb: bool, signed: bool) -> Found {
    let (dir, _m) = payload(r, version, variant, sb, 7);
    if signed {
        sign_pack(&dir, &key()).unwrap();
    }
    read_pack(&dir, c.key()).unwrap()
}

fn boot_hashes(r: &Rig) -> Vec<(String, String)> {
    tree_hash(&r.boot.root, &[])
}

fn data_tree(r: &Rig) -> Vec<(String, String)> {
    tree_hash(r.dm.data_dir(), &["devmode", "boot-next", "boot-prev", "boot-rollback", TRIAL_FILE])
}

#[test]
fn packs_are_found_in_the_folder_and_two_levels_down_newest_first_and_signatures_are_judged() {
    let r = rig("c-find");
    let c = centre(&r);
    let stick = r.dir.join("stick");
    let (d1, _) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let (d2, _) = payload(&r, "0.1.0-alpha.3", "macweb", false, 7);
    std::fs::create_dir_all(stick.join("iyakki-update")).unwrap();
    std::fs::rename(&d1, stick.join("iyakki-update/a2")).unwrap();
    std::fs::rename(&d2, stick.join("a3")).unwrap();
    sign_pack(&stick.join("a3"), &key()).unwrap();
    std::fs::write(stick.join("notes.txt"), "x").unwrap();
    let v = find_packs(&stick, c.key());
    assert_eq!(v.iter().map(|f| f.manifest.version.as_str()).collect::<Vec<_>>(), ["0.1.0-alpha.3", "0.1.0-alpha.2"]);
    assert_eq!(v[0].trust, Trust::Signed);
    assert_eq!(v[1].trust, Trust::Unsigned);
    // a manifest changed after signing, or a signature by another key, is bad
    let m = stick.join("a3/update.json");
    let mut t = std::fs::read_to_string(&m).unwrap();
    t.push(' ');
    std::fs::write(&m, t).unwrap();
    assert!(matches!(read_pack(&stick.join("a3"), c.key()).unwrap().trust, Trust::Bad(_)));
    let other = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    sign_pack(&stick.join("a3"), &other).unwrap();
    assert!(matches!(read_pack(&stick.join("a3"), c.key()).unwrap().trust, Trust::Bad(_)));
    // a folder with nothing, or a link, finds nothing
    assert!(find_packs(&r.dir.join("empty-nowhere"), c.key()).is_empty());
}

#[test]
fn a_signed_update_installs_keeps_the_previous_version_and_touches_nothing_else() {
    let r = rig("c-install");
    let c = centre(&r);
    let before = boot_hashes(&r);
    let data_before = data_tree(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    let pv = c.preview(&f).unwrap();
    assert!(pv.confirm.is_empty(), "{:?}", pv.confirm);
    assert_eq!((pv.from.as_str(), pv.to.as_str(), pv.trust.clone()), ("0.1.0-alpha.1", "0.1.0-alpha.2", Trust::Signed));
    assert!(pv.changes >= 4 && pv.same == 1 && pv.bytes > 0);
    let text = pv.lines().join("\n");
    assert!(text.contains("0.1.0-alpha.1 -> 0.1.0-alpha.2") && text.contains("Your files, accounts and settings are not touched") && text.contains("Signed"), "{text}");
    assert_eq!(boot_hashes(&r), before, "a preview writes nothing");
    let msg = c.install(&f, false, "admin").unwrap();
    assert!(msg.contains("Restart when you are ready") && msg.contains("does not restart by itself"), "{msg}");
    for fe in &f.manifest.files {
        if fe.path != "boot/grub/grub.cfg" {
            assert_eq!(hash(&find_ci(&r.boot.root, &fe.path).unwrap()), fe.sha256, "{}", fe.path);
        }
    }
    let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains(PREV_TITLE), "the menu keeps the previous version");
    assert_eq!(data_tree(&r), data_before, "the person's data is untouched");
    let v = c.versions();
    assert_eq!((v.on_disk.as_str(), v.running.as_str(), v.previous.as_deref()), ("0.1.0-alpha.2", "0.1.0-alpha.1", Some("0.1.0-alpha.1")));
    let t = read_trial(r.dm.data_dir()).unwrap();
    assert_eq!((t.state.as_str(), t.kind.as_str(), t.to.as_str()), ("restart", "update", "0.1.0-alpha.2"));
    assert!(!r.dm.data_dir().join("boot-next").join("boot").exists(), "staged files are removed");
    // it is not offered again
    assert!(c.preview(&f).is_ok());
}

#[test]
fn another_variant_an_older_version_or_no_signature_need_a_yes_and_a_wrong_signature_never_installs() {
    let r = rig("c-confirm");
    let c = centre(&r);
    let before = boot_hashes(&r);
    for (ver, variant, signed, word) in [("0.1.0-alpha.2", "pc", true, "pc image"), ("0.0.9", "macweb", true, "older"), ("0.1.0-alpha.2", "macweb", false, "no signature")] {
        let f = found(&r, &c, ver, variant, false, signed);
        let pv = c.preview(&f).unwrap();
        assert!(!pv.confirm.is_empty() && pv.confirm.join(" ").contains(word), "{ver} {variant}: {:?}", pv.confirm);
        let e = c.install(&f, false, "admin").unwrap_err();
        assert!(e.contains("needs your confirmation"), "{e}");
        assert_eq!(boot_hashes(&r), before, "{ver} {variant}: refused before any write");
        assert!(!r.dm.data_dir().join("boot-next").join("boot").exists() && !r.dm.data_dir().join("boot-prev").exists());
    }
    // with the yes the unsigned one goes in
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, false);
    c.install(&f, true, "admin").unwrap();
    // a wrong signature is refused even with a yes
    let r2 = rig("c-badsig");
    let c2 = centre(&r2);
    let before2 = boot_hashes(&r2);
    let f = found(&r2, &c2, "0.1.0-alpha.2", "macweb", false, true);
    std::fs::write(f.dir.join("update.json.sig"), "00".repeat(64)).unwrap();
    let f = read_pack(&f.dir, c2.key()).unwrap();
    assert!(matches!(f.trust, Trust::Bad(_)));
    assert!(c2.preview(&f).unwrap_err().contains("signature"));
    assert!(c2.install(&f, true, "admin").unwrap_err().contains("signature"));
    assert_eq!(boot_hashes(&r2), before2);
}

#[test]
fn a_changed_secure_boot_chain_is_refused_even_with_a_yes() {
    let r = rig("c-sb");
    let c = centre(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", true, true);
    let e = c.preview(&f).unwrap_err();
    assert!(e.contains("Secure Boot"), "{e}");
    assert!(c.install(&f, true, "admin").unwrap_err().contains("Secure Boot"));
}

#[test]
fn a_damaged_payload_is_found_before_anything_is_written() {
    let r = rig("c-damaged");
    let c = centre(&r);
    let before = boot_hashes(&r);
    let data_before = data_tree(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    let p = f.dir.join("boot/initrd");
    let mut b = std::fs::read(&p).unwrap();
    b[1000] ^= 0xff;
    std::fs::write(&p, b).unwrap();
    let e = c.install(&f, false, "admin").unwrap_err();
    assert!(e.contains("does not match its SHA-256"), "{e}");
    assert_eq!(boot_hashes(&r), before);
    assert_eq!(data_tree(&r), data_before);
    assert!(!r.dm.data_dir().join("boot-next").exists() && !r.dm.data_dir().join("boot-prev").exists(), "not even staged");
    // a file that is missing or shorter
    std::fs::remove_file(&p).unwrap();
    assert!(c.install(&f, false, "admin").is_err());
    assert_eq!(boot_hashes(&r), before);
}

#[test]
fn a_pack_changed_on_the_drive_between_the_preview_and_the_click_is_refused() {
    let r = rig("c-swap");
    let c = centre(&r);
    let before = boot_hashes(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    c.preview(&f).unwrap();
    // someone swaps the manifest (and has no key, so the signature no longer fits)
    let mut m = f.manifest.clone();
    m.version = "0.1.0-alpha.9".into();
    write_manifest(&f.dir, &m).unwrap();
    let e = c.install(&f, true, "admin").unwrap_err();
    assert!(e.contains("changed since it was shown") || e.contains("signature"), "{e}");
    assert_eq!(boot_hashes(&r), before);
}

#[test]
fn rollback_puts_the_previous_version_back_and_the_version_left_stays_for_rolling_forward() {
    let r = rig("c-rollback");
    let c = centre(&r);
    let original = boot_hashes(&r);
    let data_before = data_tree(&r);
    assert!(c.rollback("admin").unwrap_err().contains("nothing to roll back"), "no previous version yet");
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    c.install(&f, false, "admin").unwrap();
    let new_hashes = boot_hashes(&r);
    let msg = c.rollback("admin").unwrap();
    assert!(msg.contains("0.1.0-alpha.1") && msg.contains("does not restart by itself"), "{msg}");
    // the kernel and initramfs are the original ones again
    for rel in ["boot/bzImage", "boot/initrd"] {
        let want = original.iter().find(|(p, _)| p == rel).unwrap();
        assert_eq!(hash(&find_ci(&r.boot.root, rel).unwrap()), want.1, "{rel}");
    }
    let v = c.versions();
    assert_eq!((v.on_disk.as_str(), v.previous.as_deref()), ("0.1.0-alpha.1", Some("0.1.0-alpha.2")), "the bad version is the one kept now");
    let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains(&format!("{PREV_TITLE} 0.1.0-alpha.2")) && cfg.matches(PREV_TITLE).count() == 1, "{cfg}");
    assert_eq!(read_trial(r.dm.data_dir()).unwrap().kind, "rollback");
    assert_eq!(data_tree(&r), data_before, "the data volume is untouched by the rollback");
    assert!(!r.dm.data_dir().join("boot-rollback").exists());
    // and forward again
    c.rollback("admin").unwrap();
    for rel in ["boot/bzImage", "boot/initrd"] {
        let want = new_hashes.iter().find(|(p, _)| p == rel).unwrap();
        assert_eq!(hash(&find_ci(&r.boot.root, rel).unwrap()), want.1, "{rel} forward");
    }
}

#[test]
fn with_an_entry_override_a_power_cut_leaves_the_menu_on_the_version_to_restore() {
    let r = rig("c-rollcut");
    let c = centre(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    c.install(&f, false, "admin").unwrap();
    let t = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).unwrap();
    let (good_id, good_ver) = t.trim().split_once('\n').map(|(a, b)| (a.to_string(), b.to_string())).unwrap();
    // an update to a third version, cut in the middle of the initramfs, with the entry pinned to the saved first version
    let (dir, m) = payload(&r, "0.1.0-alpha.3", "macweb", false, 6);
    let free = || 500u64 << 20;
    let left = std::sync::atomic::AtomicU64::new(3 << 20);
    let e = env(&r, &free, Some(&left));
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    assert!(apply_with_entry(&e, &m, false, "admin", Some((good_id.clone(), good_ver.clone()))).unwrap_err().contains("power was cut"));
    let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains(&format!("boot-prev/{good_id}/bzImage")), "the entry starts the version to restore");
    assert!(r.dm.data_dir().join("boot-prev").join(&good_id).join("initrd").is_file());
}

#[test]
fn rollback_is_refused_while_an_update_is_unfinished() {
    let r = rig("c-rollint");
    let c = centre(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    c.install(&f, false, "admin").unwrap();
    std::fs::create_dir_all(r.dm.data_dir().join("boot-next")).unwrap();
    let mk = Marker { from: "0.1.0-alpha.1".into(), to: "0.1.0-alpha.2".into(), started: 1, prev_id: "x".into(), files: vec![] };
    std::fs::write(r.dm.data_dir().join("boot-next/UPDATING.json"), serde_json::to_vec(&mk).unwrap()).unwrap();
    let before = boot_hashes(&r);
    assert!(c.rollback("admin").unwrap_err().contains("did not finish"));
    assert_eq!(boot_hashes(&r), before);
    assert!(c.versions().interrupted.is_some());
}

#[test]
fn the_check_after_a_start_marks_the_new_version_good_or_records_the_fall_back() {
    let r = rig("c-boot");
    let c = centre(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    c.install(&f, false, "admin").unwrap();
    let data: &Path = r.dm.data_dir();
    let mut t = read_trial(data).unwrap();
    t.boot_id = "boot-a".into();
    std::fs::write(data.join(TRIAL_FILE), serde_json::to_vec(&t).unwrap()).unwrap();
    // not restarted yet: the same boot id says nothing
    assert!(after_boot(data, "0.1.0-alpha.1", "boot-a").is_none());
    assert_eq!(read_trial(data).unwrap().state, "restart");
    // restarted into the old version (the boot menu fell back): recorded once
    let msg = after_boot(data, "0.1.0-alpha.1", "boot-b").unwrap();
    assert!(msg.contains("fell back") && msg.contains("0.1.0-alpha.2"), "{msg}");
    assert_eq!(read_trial(data).unwrap().state, "fell-back");
    assert!(after_boot(data, "0.1.0-alpha.1", "boot-c").is_none());
    // the new version runs: good
    t.state = "restart".into();
    std::fs::write(data.join(TRIAL_FILE), serde_json::to_vec(&t).unwrap()).unwrap();
    let msg = after_boot(data, "0.1.0-alpha.2", "boot-b").unwrap();
    assert!(msg.contains("marked good") && msg.contains("0.1.0-alpha.1"), "{msg}");
    assert_eq!(read_trial(data).unwrap().state, "good");
}

#[test]
fn nothing_outside_the_boot_files_and_the_engines_folders_is_written() {
    let r = rig("c-scope");
    let c = centre(&r);
    let f = found(&r, &c, "0.1.0-alpha.2", "macweb", false, true);
    let top = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.path()).collect();
        v.sort();
        v
    };
    let outside_before = top(&r.dir);
    let data_top: Vec<String> = top(r.dm.data_dir()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    c.install(&f, false, "admin").unwrap();
    c.rollback("admin").unwrap();
    assert_eq!(top(&r.dir), outside_before, "no new folder beside the boot partition, the data folder and the pack");
    let after: Vec<String> = top(r.dm.data_dir()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    let new: Vec<&String> = after.iter().filter(|n| !data_top.contains(n)).collect();
    for n in &new {
        assert!(["boot-prev", "devmode", TRIAL_FILE].contains(&n.as_str()), "unexpected new entry {n}");
    }
}

#[test]
fn a_volatile_session_or_no_room_on_the_boot_partition_is_a_plain_refusal() {
    let r = rig("c-refuse");
    let ram = Centre::new(r.dm.data_dir(), Box::new(DirBoot { root: r.boot.root.clone(), capacity: 14 << 20, cluster: 4096 }), r.release.clone(), true, None).with_data_free(500 << 20);
    let f = found(&r, &ram, "0.1.0-alpha.2", "macweb", false, false);
    assert!(ram.preview(&f).unwrap_err().contains("previous version could not be kept"));
    let c = centre(&r);
    let (dir, _) = payload(&r, "0.1.0-alpha.3", "macweb", false, 20);
    let big = read_pack(&dir, c.key()).unwrap();
    assert!(c.preview(&big).unwrap_err().contains("boot partition"), "20 MB initramfs on a 14 MB partition");
}
