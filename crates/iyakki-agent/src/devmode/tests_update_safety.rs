//! Upgrade safety of `iyakki dev update` (the staged payload and its apply step; v1.0.0 gate item 5): an update touches the boot
//! files and its own two folders on the data volume and nothing else, keeps the previous version for the boot menu, refuses a
//! payload that is wrong (checksum, variant) before it writes anything, and tells an unfinished update from a clean state.
//! (`iyakki-usb`'s `bootpack` is read-only on the image: see `bootpack::tests`.)

use std::sync::atomic::AtomicU64;

use super::tests_update::{env, hash, payload, put, rig, stage_all, tree_hash, Rig};
use super::update::*;

/// What a person's data volume holds that an update must never touch (synthetic).
fn fill_data(r: &Rig) {
    let d = r.dm.data_dir();
    put(d, "firewall.json", br#"{"config":{"version":1,"enabled":true,"rules":[]},"resolved":{}}"#);
    put(d, "audit.log", b"{\"ts\":1,\"user\":\"admin\",\"event\":\"login\",\"detail\":\"\",\"prev\":\"0\",\"hash\":\"x\"}\n");
    put(d, "inbox/amy.json", br#"{"version":2,"notifications":[],"tasks":[]}"#);
    put(d, "dash/sources-amy.json", br#"[]"#);
    put(d, "flows/amy/f1/meta.json", br#"{"published":1}"#);
    put(d, "k3s-dev/state.json", br#"{"version":"v1.30"}"#);
    put(d, "home/amy/Documents/a.txt", b"synthetic document");
}

const OURS: [&str; 3] = ["devmode", "boot-next", "boot-prev"];

fn data_tree(r: &Rig) -> Vec<(String, String)> {
    tree_hash(r.dm.data_dir(), &OURS)
}

fn top_level(r: &Rig) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(r.dm.data_dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn an_update_changes_boot_files_and_its_own_folders_and_nothing_else_and_keeps_the_previous_version() {
    let r = rig("safe-touch");
    fill_data(&r);
    let before_data = data_tree(&r);
    let before_top = top_level(&r);
    let before_boot = tree_hash(&r.boot.root, &[]);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "laptop").unwrap();
    // the data volume: every file a person made is byte for byte the same, and the only new folders are the update's own
    assert_eq!(data_tree(&r), before_data);
    let new_top: Vec<String> = top_level(&r).into_iter().filter(|n| !before_top.contains(n)).collect();
    assert!(new_top.iter().all(|n| OURS.contains(&n.as_str())), "new entries on the data volume: {new_top:?}");
    // the boot partition: only paths under boot/ and EFI/ exist, and every changed or new one is in the payload
    let after_boot = tree_hash(&r.boot.root, &[]);
    for (path, h) in &after_boot {
        assert!(path.starts_with("boot/") || path.starts_with("EFI/"), "{path} is outside boot/ and EFI/");
        if before_boot.iter().find(|(p, _)| p == path).map(|x| &x.1) != Some(h) {
            assert!(m.files.iter().any(|f| f.path.eq_ignore_ascii_case(path)), "{path} changed but is not in the payload");
        }
    }
    // the previous version stays for the boot menu, complete, on the data volume (not on the partition that was rewritten)
    let id = std::fs::read_to_string(r.dm.data_dir().join("boot-prev/latest")).unwrap().lines().next().unwrap().to_string();
    for (name, from) in [("bzImage", "boot/bzImage"), ("initrd", "boot/initrd")] {
        assert_eq!(hash(&r.dm.data_dir().join("boot-prev").join(&id).join(name)), before_boot.iter().find(|(p, _)| p == from).unwrap().1);
    }
    let cfg = std::fs::read_to_string(find_ci(&r.boot.root, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains(PREV_TITLE) && cfg.contains(&format!("boot-prev/{id}/bzImage")));
    assert!(interrupted(r.dm.data_dir()).is_none() && !r.boot.root.join(MARKER).exists(), "a finished update leaves no marker");
}

#[test]
fn a_staged_file_that_does_not_match_its_checksum_stops_the_update_before_anything_is_written() {
    let r = rig("safe-sum");
    fill_data(&r);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    let boot_before = tree_hash(&r.boot.root, &[]);
    let data_before = data_tree(&r);
    // one bit flips in the staged initramfs (same length: only the checksum can tell)
    let staged = r.dm.data_dir().join("boot-next/boot/initrd");
    let mut b = std::fs::read(&staged).unwrap();
    b[1000] ^= 1;
    std::fs::write(&staged, b).unwrap();
    let err = apply(&e, &m, false, "laptop").unwrap_err();
    assert!(err.contains("not staged completely"), "{err}");
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before, "the boot partition is exactly as it was");
    assert_eq!(data_tree(&r), data_before);
    assert!(!r.boot.root.join(MARKER).exists() && interrupted(r.dm.data_dir()).is_none(), "nothing was started, so nothing is unfinished");
    assert!(!r.dm.data_dir().join("boot-prev").exists(), "not even the copy of the old version was made");
    // a staged file cut short is refused the same way
    let full = std::fs::read(dir.join("boot/initrd")).unwrap();
    std::fs::write(&staged, &full[..full.len() / 2]).unwrap();
    assert!(apply(&e, &m, false, "laptop").unwrap_err().contains("not staged completely"));
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    // the bytes sent are checked as they arrive too: a chunk that makes the file's checksum wrong is refused at the last chunk
    let r2 = rig("safe-sum2");
    let (dir2, mut m2) = payload(&r2, "0.1.0-alpha.2", "macweb", false, 7);
    let e2 = env(&r2, &free, None);
    let victim = m2.files.iter_mut().find(|f| f.path == "boot/bzImage").unwrap();
    victim.sha256 = sha256_hex(b"something else");
    let sent = std::fs::read(dir2.join("boot/bzImage")).unwrap();
    let boot_before2 = tree_hash(&r2.boot.root, &[]);
    let staged_ok = stage_chunk(&e2, &m2, "boot/bzImage", 0, &sent);
    assert!(staged_ok.is_err(), "a file that does not hash to the manifest is not accepted into staging");
    assert_eq!(tree_hash(&r2.boot.root, &[]), boot_before2);
}

#[test]
fn a_payload_for_another_variant_is_refused_and_nothing_is_written_even_when_it_was_staged() {
    let r = rig("safe-variant");
    fill_data(&r);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "pc", false, 7);
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let err = plan(&e, &m, false).unwrap_err();
    assert!(err.contains("pc") && err.contains("macweb"), "{err}");
    // someone stages the files anyway (the staging step does not decide) and calls apply: the device decides again
    stage_all(&e, &dir, &m, &m.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>());
    let boot_before = tree_hash(&r.boot.root, &[]);
    let data_before = data_tree(&r);
    assert!(apply(&e, &m, false, "laptop").is_err());
    assert_eq!(tree_hash(&r.boot.root, &[]), boot_before);
    assert_eq!(data_tree(&r), data_before);
    assert!(!r.boot.root.join(MARKER).exists() && !r.dm.data_dir().join("boot-prev").exists());
}

#[test]
fn a_payload_cut_while_it_arrives_leaves_the_old_boot_alone_and_is_resumed_not_called_unfinished() {
    let r = rig("safe-stage-cut");
    fill_data(&r);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    assert!(p.needs.len() >= 3);
    let boot_before = tree_hash(&r.boot.root, &[]);
    // the link or the power goes after N files, for every N
    for n in 0..p.needs.len() {
        let r = rig(&format!("safe-stage-cut-{n}"));
        let e = env(&r, &free, None);
        let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
        let p = plan(&e, &m, false).unwrap();
        stage_all(&e, &dir, &m, &p.needs[..n]);
        let boot_n = tree_hash(&r.boot.root, &[]);
        let err = apply(&e, &m, false, "laptop").unwrap_err();
        assert!(err.contains("not staged completely"), "{n}: {err}");
        assert_eq!(tree_hash(&r.boot.root, &[]), boot_n, "{n}: the old boot is intact");
        // nothing was started: the status says there is no unfinished update (the CLI prints "No unfinished update.")
        let s = status(r.dm.data_dir());
        assert!(s["interrupted"].is_null() && s["message"].is_null(), "{n}: {s}");
        // the plan after the cut asks only for what is missing, and the rest of the files finish it
        let again = plan(&e, &m, false).unwrap();
        stage_all(&e, &dir, &m, &again.needs);
        apply(&e, &m, false, "laptop").unwrap_or_else(|e| panic!("{n}: resume failed: {e}"));
        assert!(interrupted(r.dm.data_dir()).is_none());
    }
    let _ = (dir, boot_before);
}

#[test]
fn a_cut_while_the_boot_files_are_replaced_is_reported_as_unfinished_and_the_next_start_can_resume_it() {
    let r = rig("safe-apply-cut");
    fill_data(&r);
    let data_before = data_tree(&r);
    let (dir, m) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    let free = || 500u64 << 20;
    // power goes in the middle of the initramfs write
    let left = AtomicU64::new(3 << 20);
    let e = env(&r, &free, Some(&left));
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    assert!(apply(&e, &m, false, "laptop").unwrap_err().contains("power was cut"));
    let s = status(r.dm.data_dir());
    assert!(!s["interrupted"].is_null(), "{s}");
    let text = s["message"].as_str().unwrap();
    assert!(text.contains("did not finish") && text.contains(PREV_TITLE) && text.contains("0.1.0-alpha.2"), "{text}");
    assert_eq!(s["previous_version"], "0.1.0-alpha.1");
    assert_eq!(data_tree(&r), data_before, "the person's data is untouched by the cut");
    // power is back: the same payload resumes and finishes; the marker is gone
    let e2 = env(&r, &free, None);
    let p2 = plan(&e2, &m, false).unwrap();
    stage_all(&e2, &dir, &m, &p2.needs);
    apply(&e2, &m, false, "laptop").unwrap();
    assert!(status(r.dm.data_dir())["interrupted"].is_null());
    assert_eq!(data_tree(&r), data_before);
}

/// A payload with a new loader (and its config) in the boot loader's folder.
fn loader_payload(r: &Rig, loader: &[u8]) -> (std::path::PathBuf, Manifest) {
    let (dir, _) = payload(r, "0.1.0-alpha.2", "macweb", false, 7);
    put(&dir, "EFI/iyakki-boot/loader.efi", loader);
    put(&dir, "EFI/iyakki-boot/boot.cfg", b"timeout 3\nentry a\n kernel /boot/bzImage\n");
    let list = list_boot(&dir).into_iter().filter(|(p, _)| p != MANIFEST).map(|(p, s)| (p.clone(), s, super::files::sha256_file(&dir.join(&p)).unwrap())).collect::<Vec<_>>();
    let m = build_manifest(&dir, &list, None, None, "test.img").unwrap();
    write_manifest(&dir, &m).unwrap();
    (dir, m)
}

#[test]
fn an_update_keeps_the_previous_boot_loader_beside_the_new_one() {
    let r = rig("loader-prev");
    put(&r.boot.root, "EFI/iyakki-boot/loader.efi", b"OLD LOADER");
    put(&r.boot.root, "EFI/iyakki-boot/boot.cfg", b"timeout 1\n");
    let (dir, m) = loader_payload(&r, b"NEW LOADER");
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "laptop").unwrap();
    let d = r.boot.root.join("EFI/iyakki-boot");
    assert_eq!(std::fs::read(d.join("loader.efi")).unwrap(), b"NEW LOADER");
    assert_eq!(std::fs::read(d.join("loader.prev.efi")).unwrap(), b"OLD LOADER", "the loader the menu offers as the previous version");
    assert!(std::fs::read_to_string(d.join("boot.cfg")).unwrap().contains("entry a"));
    let left: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".upd")).collect();
    assert!(left.is_empty(), "no temporary files stay behind: {left:?}");
}

#[test]
fn a_power_cut_while_the_loader_is_written_leaves_the_old_loader_whole() {
    let free = || 500u64 << 20;
    // the loader is written near the end of an update (after the kernel and initramfs); sweep the cut over the whole write budget
    let total: u64 = 2 * (7 << 20) + 4 * (1 << 20) + 400_000;
    let mut saw_cut_with_old_loader = false;
    for k in (0..total).step_by((total / 40).max(1) as usize) {
        let r2 = rig(&format!("loader-cut-{k}"));
        put(&r2.boot.root, "EFI/iyakki-boot/loader.efi", b"OLD LOADER");
        let (dir2, m2) = loader_payload(&r2, &vec![7u8; 90_000]);
        let left = AtomicU64::new(k);
        let e2 = env(&r2, &free, Some(&left));
        let p2 = plan(&e2, &m2, false).unwrap();
        stage_all(&e2, &dir2, &m2, &p2.needs);
        if apply(&e2, &m2, false, "laptop").is_err() {
            let now = std::fs::read(r2.boot.root.join("EFI/iyakki-boot/loader.efi")).unwrap();
            assert!(now == b"OLD LOADER" || now == vec![7u8; 90_000], "cut at {k}: the loader is half written ({} bytes)", now.len());
            saw_cut_with_old_loader |= now == b"OLD LOADER";
        }
    }
    assert!(saw_cut_with_old_loader);
}

#[test]
fn the_loader_folder_is_recognised() {
    assert!(in_loader_dir("EFI/iyakki-boot/boot.cfg") && in_loader_dir("efi/IYAKKI-BOOT/loader.efi"));
    assert!(!in_loader_dir("EFI/BOOT/bootx64.efi") && !in_loader_dir("EFI/iyakki-boot") && !in_loader_dir("boot/iyakki-boot/x"));
}

/// A shim-chain boot volume (shim, MokManager, `grubx64.efi` = GRUB) and a payload of a loader-default image: `grubx64.efi` is the loader,
/// `grub-recovery.efi` is the GRUB, and the loader's folder is there. `loader_default` false makes the GRUB-default payload (the way back).
fn chain_payload(r: &Rig, version: &str, loader_default: bool, grub: &[u8], loader: &[u8]) -> (std::path::PathBuf, Manifest) {
    let (dir, _) = payload(r, version, "macweb", true, 7);
    put(&dir, "EFI/BOOT/grubx64.efi", if loader_default { loader } else { grub });
    if loader_default {
        put(&dir, "EFI/BOOT/grub-recovery.efi", grub);
        put(&dir, "EFI/iyakki-boot/loader.efi", loader);
        put(&dir, "EFI/iyakki-boot/boot.cfg", b"timeout 3\nentry recovery\n kind chain\n kernel /EFI/BOOT/grub-recovery.efi\n");
    }
    let list = list_boot(&dir).into_iter().filter(|(p, _)| p != MANIFEST).map(|(p, s)| (p.clone(), s, super::files::sha256_file(&dir.join(&p)).unwrap())).collect::<Vec<_>>();
    let m = build_manifest(&dir, &list, None, None, "test.img").unwrap();
    write_manifest(&dir, &m).unwrap();
    (dir, m)
}

fn chain_rig(tag: &str, grub: &[u8]) -> Rig {
    let r = rig(tag);
    put(&r.boot.root, "EFI/BOOT/grubx64.efi", grub);
    put(&r.boot.root, "EFI/BOOT/mmx64.efi", &super::tests_update::pseudo(1000, 4));
    r
}

/// The first move of an installed GRUB-default image to a loader-default one: `grubx64.efi` (shim's second stage) is swapped from GRUB to the
/// loader and GRUB appears as `grub-recovery.efi`. Whatever moment the power goes, the second stage is one whole file (GRUB or the loader), and
/// as soon as it is the loader, GRUB's recovery copy is whole beside it.
#[test]
fn the_first_move_to_a_loader_default_image_never_leaves_the_second_stage_broken_or_without_recovery() {
    let free = || 500u64 << 20;
    let grub = vec![0x47u8; 400_000];
    let loader = vec![0x4cu8; 90_000];
    let total: u64 = 2 * (7 << 20) + 4 * (1 << 20) + 1_000_000;
    let (mut saw_old, mut saw_new) = (false, false);
    for k in (0..total).step_by((total / 60).max(1) as usize).chain([u64::MAX / 4]) {
        let r = chain_rig(&format!("chain-cut-{k}"), &grub);
        let (dir, m) = chain_payload(&r, "0.1.0-alpha.2", true, &grub, &loader);
        let left = AtomicU64::new(k);
        let e = env(&r, &free, Some(&left));
        let p = plan(&e, &m, false).unwrap();
        assert!(p.needs.iter().any(|n| n == "EFI/BOOT/grub-recovery.efi") && p.needs.iter().any(|n| n == "EFI/BOOT/grubx64.efi"), "{:?}", p.needs);
        stage_all(&e, &dir, &m, &p.needs);
        let res = apply(&e, &m, false, "laptop");
        if k > total {
            res.as_ref().unwrap();
        }
        let second = std::fs::read(r.boot.root.join("EFI/BOOT/grubx64.efi")).unwrap();
        assert!(second == grub || second == loader, "cut at {k}: the second stage is half written ({} bytes)", second.len());
        if second == loader {
            saw_new = true;
            assert_eq!(std::fs::read(r.boot.root.join("EFI/BOOT/grub-recovery.efi")).unwrap(), grub, "cut at {k}: the loader is the second stage but GRUB's recovery copy is not whole");
            assert!(r.boot.root.join("EFI/iyakki-boot/boot.cfg").is_file() && r.boot.root.join("EFI/iyakki-boot/loader.efi").is_file(), "cut at {k}: loader files missing");
        } else {
            saw_old = true;
        }
        // a recovery copy, when present, is whole (written beside and renamed), and no temporary file is named like a boot program
        if let Ok(rec) = std::fs::read(r.boot.root.join("EFI/BOOT/grub-recovery.efi")) {
            assert_eq!(rec, grub, "cut at {k}: grub-recovery.efi is half written");
        }
        assert_eq!(std::fs::read(r.boot.root.join("EFI/BOOT/mmx64.efi")).unwrap(), super::tests_update::pseudo(1000, 4));
        if res.is_ok() {
            let left: Vec<String> = ["EFI/BOOT", "EFI/iyakki-boot"].iter().flat_map(|d| std::fs::read_dir(r.boot.root.join(d)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).filter(|n| n.ends_with(".upd")).collect();
            assert!(left.is_empty(), "{left:?}");
        }
    }
    assert!(saw_old && saw_new, "the sweep must see a cut before and after the swap");
}

/// The way back: a GRUB-default pack puts GRUB on `grubx64.efi` again (atomically); the recovery copy and the loader's folder stay.
#[test]
fn a_grub_default_pack_puts_grub_back_as_the_second_stage_and_leaves_the_loader_files() {
    let free = || 500u64 << 20;
    let grub = vec![0x47u8; 400_000];
    let loader = vec![0x4cu8; 90_000];
    let r = chain_rig("chain-back", &loader);
    put(&r.boot.root, "EFI/BOOT/grub-recovery.efi", &grub);
    put(&r.boot.root, "EFI/iyakki-boot/loader.efi", &loader);
    let (dir, m) = chain_payload(&r, "0.1.0-alpha.2", false, &grub, &loader);
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    stage_all(&e, &dir, &m, &p.needs);
    apply(&e, &m, false, "laptop").unwrap();
    assert_eq!(std::fs::read(r.boot.root.join("EFI/BOOT/grubx64.efi")).unwrap(), grub);
    assert_eq!(std::fs::read(r.boot.root.join("EFI/BOOT/grub-recovery.efi")).unwrap(), grub);
    assert_eq!(std::fs::read(r.boot.root.join("EFI/iyakki-boot/loader.efi")).unwrap(), loader);
}

#[test]
fn the_boot_path_files_are_recognised_and_the_recovery_copy_is_written_before_the_second_stage() {
    assert!(in_boot_chain("EFI/BOOT/grubx64.efi") && in_boot_chain("efi/boot/BOOTX64.EFI") && in_boot_chain("EFI/BOOT/grub-recovery.efi"));
    assert!(!in_boot_chain("EFI/iyakki-boot/loader.efi") && !in_boot_chain("EFI/BOOT") && !in_boot_chain("boot/grub/grub.cfg"));
    let r = chain_rig("chain-order", &[1u8; 1000]);
    let (_, m) = chain_payload(&r, "0.1.0-alpha.2", true, &[1u8; 1000], &[2u8; 500]);
    let free = || 500u64 << 20;
    let e = env(&r, &free, None);
    let p = plan(&e, &m, false).unwrap();
    // the writes are logged in order by `apply`; here the rule itself: recovery, loader folder, others, then the by-name files
    let rank = |n: &str| super::update::efi_write_rank_for_tests(n);
    assert!(rank("EFI/BOOT/grub-recovery.efi") < rank("EFI/iyakki-boot/boot.cfg"));
    assert!(rank("EFI/iyakki-boot/loader.efi") < rank("EFI/BOOT/grubx64.efi") && rank("EFI/iyakki-boot/loader.efi") < rank("EFI/BOOT/BOOTX64.EFI"));
    assert!(rank("EFI/BOOT/mmx64.efi") < rank("EFI/BOOT/grubx64.efi"));
    assert!(p.needs.len() >= 4);
}
