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
