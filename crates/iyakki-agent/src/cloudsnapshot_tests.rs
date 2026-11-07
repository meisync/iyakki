use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-snapshot-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tenant data folder with files for two people, an empty folder and (on unix) a link that must not be archived.
fn tenant(tag: &str) -> (PathBuf, PathBuf) {
    let data = dir(tag);
    let files = data.join("cloud/files");
    std::fs::create_dir_all(files.join("ns/web/people/ann/empty")).unwrap();
    std::fs::create_dir_all(files.join("ns/web/shared/docs")).unwrap();
    std::fs::write(files.join("ns/web/people/ann/a.txt"), "ann's file").unwrap();
    std::fs::write(files.join("ns/web/shared/docs/readme.md"), "x".repeat(70_000)).unwrap();
    let long = format!("ns/web/shared/{}/deep.txt", "d".repeat(120));
    std::fs::create_dir_all(files.join(&long).parent().unwrap()).unwrap();
    std::fs::write(files.join(&long), "long path").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc", files.join("ns/web/people/ann/etc")).unwrap();
    (data, files)
}

#[test]
fn ids_sort_by_time_and_only_ours_are_accepted() {
    assert_eq!(id_for(0), "19700101T000000Z");
    assert_eq!(id_for(1_793_635_200), "20261102T160000Z");
    assert!(id_for(1) < id_for(2) && id_for(86_399) < id_for(86_400));
    for ok in ["20261102T160000Z", "20261102T160000Z-2"] {
        assert!(id_ok(ok), "{ok}");
    }
    for bad in ["", "..", "../x", "20261102T160000Z-", "20261102T160000Z-x", "20261102160000Z", ".making-20261102T160000Z", "20261102T160000Z/x"] {
        assert!(!id_ok(bad), "{bad}");
    }
}

#[test]
fn a_snapshot_has_a_manifest_with_checksums_and_lives_outside_the_live_folders() {
    let (data, files) = tenant("create");
    let snaps = data.join("cloud/snapshots");
    let m = create(&files, &snaps, 1_793_635_200).unwrap();
    assert_eq!(m.id, "20261102T160000Z");
    assert_eq!(m.files(), 3);
    let a = m.items.iter().find(|i| i.path == "ns/web/people/ann/a.txt").unwrap();
    assert_eq!((a.size, a.sha256.as_str()), (10, crate::ocipush::sha256_hex(b"ann's file").as_str()));
    assert!(m.items.iter().any(|i| i.path == "ns/web/people/ann/empty" && i.kind == "dir"), "empty folders are kept");
    #[cfg(unix)]
    {
        assert!(m.skipped.iter().any(|s| s.contains("ann/etc") && s.contains("link")), "{:?}", m.skipped);
        assert!(!m.items.iter().any(|i| i.path.contains("ann/etc")));
    }
    assert!(snaps.join(&m.id).join("data.tar.zst").is_file() && !snaps.join(&m.id).join("data.tar").exists());
    assert!(!snaps.starts_with(&files), "snapshots are not inside the live files");
    assert!(std::fs::read_dir(&snaps).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".making")), "no temporary folder left");
    assert_eq!(verify(&snaps, &m.id).unwrap(), m);
    // a second snapshot in the same second gets its own id
    let m2 = create(&files, &snaps, 1_793_635_200).unwrap();
    assert_eq!(m2.id, "20261102T160000Z-1");
    assert_eq!(list(&snaps).len(), 2);
    // nested folders are refused
    assert!(create(&files, &files.join("snaps"), 5).unwrap_err().contains("inside each other"));
    assert!(create(&data, &snaps, 5).is_err(), "the files folder's parent holds the snapshots");
}

#[test]
fn restore_goes_to_a_new_folder_and_comes_back_byte_for_byte() {
    let (data, files) = tenant("restore");
    let snaps = data.join("cloud/snapshots");
    let m = create(&files, &snaps, 100).unwrap();
    let to = data.join("cloud/restored/one");
    let r = restore(&snaps, &m.id, &to, false).unwrap();
    assert_eq!((r.files, r.bytes), (3, 10 + 70_000 + 9));
    assert_eq!(std::fs::read_to_string(to.join("ns/web/people/ann/a.txt")).unwrap(), "ann's file");
    assert!(to.join("ns/web/people/ann/empty").is_dir());
    assert_eq!(std::fs::read(to.join("ns/web/shared/docs/readme.md")).unwrap().len(), 70_000);
    assert!(to.join(format!("ns/web/shared/{}/deep.txt", "d".repeat(120))).is_file(), "a long path survives the tar prefix");
    assert!(!to.join("ns/web/people/ann/etc").exists(), "the link was not restored");
    assert!(std::fs::read_dir(snaps.join(&m.id)).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".restore")), "staging is gone");
}

#[test]
fn restore_never_overwrites_without_the_flag_and_writes_nothing_when_it_refuses() {
    let (data, files) = tenant("overwrite");
    let snaps = data.join("cloud/snapshots");
    let m = create(&files, &snaps, 100).unwrap();
    // live: the files are all there already
    std::fs::write(files.join("ns/web/people/ann/a.txt"), "changed since").unwrap();
    std::fs::remove_file(files.join("ns/web/shared/docs/readme.md")).unwrap();
    let e = restore(&snaps, &m.id, &files, false).unwrap_err();
    assert!(e.contains("already exist") && e.contains("nothing was written") && e.contains("--overwrite"), "{e}");
    assert_eq!(std::fs::read_to_string(files.join("ns/web/people/ann/a.txt")).unwrap(), "changed since");
    assert!(!files.join("ns/web/shared/docs/readme.md").exists(), "not even the missing file came back");
    restore(&snaps, &m.id, &files, true).unwrap();
    assert_eq!(std::fs::read_to_string(files.join("ns/web/people/ann/a.txt")).unwrap(), "ann's file");
    assert!(files.join("ns/web/shared/docs/readme.md").is_file());
}

#[cfg(unix)]
#[test]
fn restore_will_not_write_through_a_link_in_the_destination() {
    let (data, files) = tenant("link");
    let snaps = data.join("cloud/snapshots");
    let m = create(&files, &snaps, 100).unwrap();
    let to = dir("link-to");
    let outside = dir("link-outside");
    std::os::unix::fs::symlink(&outside, to.join("ns")).unwrap();
    for overwrite in [false, true] {
        let e = restore(&snaps, &m.id, &to, overwrite).unwrap_err();
        assert!(e.contains("link"), "{e}");
    }
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none(), "nothing was written behind the link");
}

#[test]
fn a_damaged_archive_a_changed_manifest_and_a_bad_id_are_refused() {
    let (data, files) = tenant("damage");
    let snaps = data.join("cloud/snapshots");
    let m = create(&files, &snaps, 100).unwrap();
    let to = data.join("out");
    // the archive changes
    let arch = snaps.join(&m.id).join("data.tar.zst");
    let mut b = std::fs::read(&arch).unwrap();
    let n = b.len() / 2;
    b[n] ^= 0xff;
    std::fs::write(&arch, &b).unwrap();
    assert!(verify(&snaps, &m.id).unwrap_err().contains("archive does not match"));
    assert!(restore(&snaps, &m.id, &to, false).is_err());
    assert!(!to.join("ns").exists(), "nothing restored from a damaged archive");
    // the manifest changes (a path pointing out of the folder, with a forged checksum of the archive left alone)
    let m2 = create(&files, &snaps, 200).unwrap();
    let mf = snaps.join(&m2.id).join("manifest.json");
    let t = std::fs::read_to_string(&mf).unwrap().replace("ns/web/people/ann/a.txt", "../../escape.txt");
    std::fs::write(&mf, &t).unwrap();
    assert!(read_manifest(&snaps, &m2.id).unwrap_err().contains("checksum"));
    // even with a matching checksum, a path that leaves the folder is not valid
    std::fs::write(snaps.join(&m2.id).join("manifest.sha256"), format!("{}  manifest.json\n", hex(&Sha256::digest(t.as_bytes())))).unwrap();
    assert!(read_manifest(&snaps, &m2.id).unwrap_err().contains("not valid"));
    assert!(restore(&snaps, "../x", &to, false).is_err() && verify(&snaps, "nope").is_err());
    let l = list(&snaps);
    assert_eq!(l.len(), 2, "a damaged snapshot is shown, not hidden");
    assert!(l[0].1.is_ok() && verify(&snaps, &l[0].0).is_err(), "the list reads manifests; verify checks the archive");
    assert!(l[1].1.is_err(), "an unreadable manifest is shown with its reason");
}

#[test]
fn prune_keeps_the_newest_never_all_and_leaves_unreadable_ones_alone() {
    let (data, files) = tenant("prune");
    let snaps = data.join("cloud/snapshots");
    let ids: Vec<String> = (1..=4).map(|i| create(&files, &snaps, i * 1000).unwrap().id).collect();
    assert!(prune(&snaps, 0, false).is_err());
    assert_eq!(prune(&snaps, 3, true).unwrap(), vec![ids[0].clone()], "a dry run removes nothing");
    assert_eq!(list(&snaps).len(), 4);
    // damage the newest manifest: it is kept, and not counted as a good one
    std::fs::write(snaps.join(&ids[3]).join("manifest.json"), "{}").unwrap();
    std::fs::create_dir_all(snaps.join("notes-by-hand")).unwrap();
    let gone = prune(&snaps, 1, false).unwrap();
    assert_eq!(gone, vec![ids[0].clone(), ids[1].clone()]);
    assert!(snaps.join(&ids[2]).is_dir() && snaps.join(&ids[3]).is_dir() && snaps.join("notes-by-hand").is_dir());
    assert!(!snaps.join(&ids[0]).exists());
}

#[test]
fn an_empty_tenant_gives_an_empty_snapshot() {
    let data = dir("empty");
    let m = create(&data.join("cloud/files"), &data.join("cloud/snapshots"), 7).unwrap();
    assert_eq!((m.files(), m.items.len()), (0, 0));
    assert!(verify(&data.join("cloud/snapshots"), &m.id).is_ok());
    let to = data.join("r");
    assert_eq!(restore(&data.join("cloud/snapshots"), &m.id, &to, false).unwrap(), Restored { files: 0, dirs: 0, bytes: 0 });
}

#[test]
fn the_command_creates_lists_verifies_restores_and_prunes() {
    let (data, _) = tenant("cli");
    let d = data.to_string_lossy().to_string();
    let run = |args: &[&str]| -> Result<String, String> {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).chain(["--data".to_string(), d.clone()]).collect();
        let mut text = String::new();
        cli(&a, &mut |l| {
            text.push_str(l);
            text.push('\n');
        })?;
        Ok(text)
    };
    let c = run(&["create"]).unwrap();
    assert!(c.starts_with("snapshot ") && c.contains("3 files"), "{c}");
    let id = c.split_whitespace().nth(1).unwrap().trim_end_matches(':').to_string();
    assert!(run(&["list"]).unwrap().contains(&id));
    assert!(run(&["verify", &id]).unwrap().contains("intact"));
    let r = run(&["restore", &id]).unwrap();
    assert!(r.contains("restored 3 files") && data.join("cloud/restored").join(&id).is_dir(), "{r}");
    assert!(run(&["restore", &id]).unwrap_err().contains("exists already"), "the default folder is never reused");
    assert!(run(&["restore", &id, "--live"]).unwrap_err().contains("already exist"));
    assert!(run(&["restore", &id, "--live", "--to", "x"]).is_err());
    assert!(run(&["restore"]).is_err() && run(&["bogus"]).is_err());
    assert!(run(&["create", "--keep", "1"]).is_ok());
    assert!(run(&["list"]).unwrap().lines().count() <= 2);
    assert!(run(&["prune", "--keep", "1", "--dry-run"]).unwrap().contains("would remove 0"));
}

#[test]
fn the_cronjob_manifest_has_the_schedule_the_security_settings_and_the_claims() {
    let y = cronjob(&CronSpec { tenant: "acme", schedule: "17 3 * * *", image: "reg.example.org/iyakki:1.2", keep: 7, backup_claim: None }).unwrap();
    for must in [
        "kind: CronJob", "namespace: iyakki-acme", "schedule: \"17 3 * * *\"", "concurrencyPolicy: Forbid", "image: reg.example.org/iyakki:1.2",
        "args: [\"cloud\", \"snapshot\", \"create\", \"--data\", \"/var/lib/iyakki\", \"--keep\", \"7\"]", "claimName: iyakki-data", "runAsNonRoot: true", "readOnlyRootFilesystem: true",
        "allowPrivilegeEscalation: false", "drop: [\"ALL\"]", "automountServiceAccountToken: false", "app.kubernetes.io/instance: acme", "kubernetes.io/hostname",
    ] {
        assert!(y.contains(must), "missing {must}\n{y}");
    }
    assert!(!y.contains("/backup"));
    let y = cronjob(&CronSpec { tenant: "acme", schedule: "*/30 * * * *", image: "iyakki:latest", keep: 3, backup_claim: Some("acme-backup") }).unwrap();
    assert!(y.contains("\"--dir\", \"/backup\"") && y.contains("claimName: acme-backup") && y.contains("mountPath: /backup"), "{y}");
    // nothing but plain values reaches the YAML
    for (t, s, i, k, c) in [
        ("Acme", "17 3 * * *", "iyakki", 7, None),
        ("a\nb", "17 3 * * *", "iyakki", 7, None),
        ("acme", "17 3 * *", "iyakki", 7, None),
        ("acme", "17 3 * * *\"; evil", "iyakki", 7, None),
        ("acme", "@daily", "iyakki", 7, None),
        ("acme", "17 3 * * *", "img\nx: y", 7, None),
        ("acme", "17 3 * * *", "-img", 7, None),
        ("acme", "17 3 * * *", "iyakki", 0, None),
        ("acme", "17 3 * * *", "iyakki", 7, Some("A b")),
    ] {
        assert!(cronjob(&CronSpec { tenant: t, schedule: s, image: i, keep: k, backup_claim: c }).is_err(), "{t:?} {s:?} {i:?} {k} {c:?}");
    }
}

// ------------------------------------------------------------------------------------------------------------ encryption at rest

fn key(n: u8) -> SnapKey {
    SnapKey::from_bytes(&[n; 32]).unwrap()
}

/// Like `tenant`, plus a 300 KB file of noise (more than four sealed chunks even after compression).
fn tenant_with_noise(tag: &str) -> (PathBuf, PathBuf) {
    let (data, files) = tenant(tag);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(files.join("ns/web/shared/noise.bin"), &noise).unwrap();
    (data, files)
}

fn ring(ks: &[u8]) -> KeyRing {
    KeyRing(ks.iter().map(|n| key(*n)).collect())
}

/// Change the archive of snapshot `id` and make the manifest agree with the new bytes, so only the authentication can notice.
fn forge(snaps: &Path, id: &str, f: impl Fn(&mut Vec<u8>)) {
    let m = read_manifest(snaps, id).unwrap();
    let arch = snaps.join(id).join(&m.archive);
    let mut b = std::fs::read(&arch).unwrap();
    f(&mut b);
    std::fs::write(&arch, &b).unwrap();
    let m2 = Manifest { archive_sha256: crate::ocipush::sha256_hex(&b), archive_bytes: b.len() as u64, ..m };
    let text = serde_json::to_string_pretty(&m2).unwrap();
    std::fs::write(snaps.join(id).join("manifest.json"), &text).unwrap();
    std::fs::write(snaps.join(id).join("manifest.sha256"), format!("{}  manifest.json\n", hex(&Sha256::digest(text.as_bytes())))).unwrap();
}

#[test]
fn an_encrypted_snapshot_holds_no_plain_names_or_contents_and_comes_back_byte_for_byte() {
    let (data, files) = tenant_with_noise("enc-roundtrip");
    let snaps = data.join("cloud/snapshots");
    let k = key(1);
    let m = create_with(&files, &snaps, 100, Some(&k)).unwrap();
    assert_eq!((m.files(), m.enc.as_ref().map(|e| e.key_id.clone())), (4, Some(k.id.clone())));
    let d = snaps.join(&m.id);
    assert!(d.join("data.tar.zst.enc").is_file() && !d.join("data.tar.zst").exists() && !d.join("data.tar").exists());
    // nothing readable at rest: not the contents, not the names, not in the manifest either
    let arch = std::fs::read(d.join("data.tar.zst.enc")).unwrap();
    let man = std::fs::read_to_string(d.join("manifest.json")).unwrap();
    for needle in ["ann's file", "a.txt", "readme", "noise.bin", "people"] {
        assert!(!arch.windows(needle.len()).any(|w| w == needle.as_bytes()), "archive shows {needle}");
        assert!(!man.contains(needle), "manifest shows {needle}");
    }
    assert!(arch.starts_with(b"IYSNAP1\n"));
    let ks = ring(&[1]);
    // listing and the checksum need no key; the content check does
    assert!(list(&snaps)[0].1.as_ref().unwrap().enc.is_some());
    assert!(verify(&snaps, &m.id).is_ok());
    let v = verify_with(&snaps, &m.id, Some(&ks)).unwrap();
    assert_eq!((v.files(), v.items.len()), (4, m.items.len()));
    assert!(restore(&snaps, &m.id, &data.join("nokey"), false).unwrap_err().contains("encrypted"));
    assert!(!data.join("nokey").exists());
    let to = data.join("cloud/restored/enc");
    let r = restore_with(&snaps, &m.id, &to, false, Some(&ks)).unwrap();
    assert_eq!(r.files, 4);
    assert_eq!(std::fs::read(to.join("ns/web/shared/noise.bin")).unwrap(), std::fs::read(files.join("ns/web/shared/noise.bin")).unwrap());
    assert_eq!(std::fs::read_to_string(to.join("ns/web/people/ann/a.txt")).unwrap(), "ann's file");
    assert!(std::fs::read_dir(snaps.join(&m.id)).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".restore")), "no plain staging left");
}

#[test]
fn the_wrong_key_opens_nothing_and_writes_nothing() {
    let (data, files) = tenant_with_noise("enc-wrong");
    let snaps = data.join("cloud/snapshots");
    let m = create_with(&files, &snaps, 100, Some(&key(1))).unwrap();
    let to = data.join("out");
    for ks in [ring(&[2]), ring(&[])] {
        let e = restore_with(&snaps, &m.id, &to, false, Some(&ks)).unwrap_err();
        assert!(e.contains("no key") || e.contains("wrong key"), "{e}");
        assert!(verify_with(&snaps, &m.id, Some(&ks)).is_err());
    }
    // a key that has the right id but is not the key cannot exist (the id is the key's hash); a manifest that names another id fails the seal
    let e = read_manifest(&snaps, &m.id).unwrap().enc.unwrap();
    assert_eq!(e.key_id, key(1).id);
    assert!(!to.exists(), "nothing was written");
    assert!(open_manifest(&snaps, &m.id, None).unwrap_err().contains("encrypted"));
}

#[test]
fn a_changed_cut_extended_or_reordered_archive_is_refused_even_when_its_checksums_are_made_to_agree() {
    let (data, files) = tenant_with_noise("enc-tamper");
    let snaps = data.join("cloud/snapshots");
    let ks = ring(&[1]);
    let chunk = CHUNK + TAG;
    let cases: Vec<(&str, Box<dyn Fn(&mut Vec<u8>)>)> = vec![
        ("a flipped bit", Box::new(|b| { let n = b.len() / 2; b[n] ^= 1 })),
        ("a flipped bit in the first chunk", Box::new(|b| b[20] ^= 0x80)),
        ("the last bytes cut off", Box::new(|b| { let n = b.len() - 5; b.truncate(n) })),
        ("a whole chunk removed from the end", Box::new(move |b| { let n = b.len().saturating_sub(chunk); b.truncate(n) })),
        ("bytes added at the end", Box::new(|b| b.extend_from_slice(b"extra"))),
        ("a chunk duplicated", Box::new(move |b| { let c = b[8..8 + chunk].to_vec(); b.splice(8 + chunk..8 + chunk, c); })),
        ("two chunks swapped", Box::new(move |b| { let (x, y) = (b[8..8 + chunk].to_vec(), b[8 + chunk..8 + 2 * chunk].to_vec()); b[8..8 + chunk].copy_from_slice(&y); b[8 + chunk..8 + 2 * chunk].copy_from_slice(&x); })),
        ("an empty file with the magic", Box::new(|b| b.truncate(8))),
        ("no magic", Box::new(|b| b[0] = b'X')),
    ];
    for (i, (what, f)) in cases.iter().enumerate() {
        let m = create_with(&files, &snaps, 100 + i as u64, Some(&key(1))).unwrap();
        assert!(verify_with(&snaps, &m.id, Some(&ks)).is_ok());
        forge(&snaps, &m.id, f);
        assert!(verify(&snaps, &m.id).is_ok(), "{what}: the plain checks were made to agree");
        let to = data.join(format!("out{i}"));
        let e = restore_with(&snaps, &m.id, &to, false, Some(&ks)).unwrap_err();
        assert!(e.contains("wrong key") || e.contains("not an encrypted"), "{what}: {e}");
        assert!(!to.exists(), "{what}: nothing restored");
        assert!(verify_with(&snaps, &m.id, Some(&ks)).is_err(), "{what}");
    }
    // the sealed file list and the key id are bound too
    let m = create_with(&files, &snaps, 500, Some(&key(1))).unwrap();
    let mp = snaps.join(&m.id).join("manifest.json");
    let text = std::fs::read_to_string(&mp).unwrap();
    let mut j: serde_json::Value = serde_json::from_str(&text).unwrap();
    let sealed = j["enc"]["sealed"].as_str().unwrap().to_string();
    let flipped = format!("{}{}", &sealed[..sealed.len() - 2], if sealed.ends_with("00") { "01" } else { "00" });
    j["enc"]["sealed"] = flipped.into();
    let t2 = serde_json::to_string_pretty(&j).unwrap();
    std::fs::write(&mp, &t2).unwrap();
    std::fs::write(snaps.join(&m.id).join("manifest.sha256"), format!("{}  manifest.json\n", hex(&Sha256::digest(t2.as_bytes())))).unwrap();
    assert!(open_manifest(&snaps, &m.id, Some(&ks)).unwrap_err().contains("wrong key"));
    // claiming another key id: the other key is found, and the seal still does not open
    let both = ring(&[1, 2]);
    let m = create_with(&files, &snaps, 600, Some(&key(1))).unwrap();
    let mp = snaps.join(&m.id).join("manifest.json");
    let mut j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&mp).unwrap()).unwrap();
    j["enc"]["key_id"] = key(2).id.clone().into();
    let t3 = serde_json::to_string_pretty(&j).unwrap();
    std::fs::write(&mp, &t3).unwrap();
    std::fs::write(snaps.join(&m.id).join("manifest.sha256"), format!("{}  manifest.json\n", hex(&Sha256::digest(t3.as_bytes())))).unwrap();
    assert!(verify_with(&snaps, &m.id, Some(&both)).is_err());
}

#[test]
fn rekey_moves_a_snapshot_to_a_new_key_and_a_failed_rekey_changes_nothing() {
    let (data, files) = tenant_with_noise("enc-rekey");
    let snaps = data.join("cloud/snapshots");
    let m = create_with(&files, &snaps, 100, Some(&key(1))).unwrap();
    let plain = create(&files, &snaps, 200).unwrap();
    let before = std::fs::read(snaps.join(&m.id).join("data.tar.zst.enc")).unwrap();
    // wrong old key: refused, the snapshot is as it was
    assert!(rekey(&snaps, &m.id, Some(&ring(&[9])), &key(2)).is_err());
    assert_eq!(std::fs::read(snaps.join(&m.id).join("data.tar.zst.enc")).unwrap(), before);
    assert!(verify_with(&snaps, &m.id, Some(&ring(&[1]))).is_ok());
    // the right one
    let r = rekey(&snaps, &m.id, Some(&ring(&[1])), &key(2)).unwrap();
    assert_eq!(r.enc.unwrap().key_id, key(2).id);
    assert!(verify_with(&snaps, &m.id, Some(&ring(&[2]))).is_ok());
    assert!(verify_with(&snaps, &m.id, Some(&ring(&[1]))).is_err(), "the old key no longer opens it");
    let to = data.join("again");
    restore_with(&snaps, &m.id, &to, false, Some(&ring(&[2]))).unwrap();
    assert_eq!(std::fs::read(to.join("ns/web/shared/noise.bin")).unwrap(), std::fs::read(files.join("ns/web/shared/noise.bin")).unwrap());
    // the same key again is a no-op; a plain snapshot can be sealed
    assert_eq!(rekey(&snaps, &m.id, Some(&ring(&[2])), &key(2)).unwrap().id, m.id);
    rekey(&snaps, &plain.id, None, &key(3)).unwrap();
    assert!(read_manifest(&snaps, &plain.id).unwrap().enc.is_some() && !snaps.join(&plain.id).join("data.tar.zst").exists());
    assert!(verify_with(&snaps, &plain.id, Some(&ring(&[3]))).is_ok());
    let names: Vec<String> = std::fs::read_dir(&snaps).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
    // a ring with several keys finds each snapshot's key by id; the first is the current one
    assert_eq!(ring(&[5, 6]).current().unwrap().id, key(5).id);
    assert!(ring(&[5, 6]).get(&key(6).id).is_ok() && ring(&[5]).get(&key(6).id).is_err());
}

#[test]
fn keys_have_ids_that_name_them_and_files_hold_hex() {
    let k = key(7);
    assert_eq!(k.id.len(), 16);
    assert_eq!(SnapKey::from_hex(&format!("{}\n", k.to_hex())).unwrap().id, k.id);
    for bad in ["", "zz", &"ab".repeat(31), &"ab".repeat(33)] {
        assert!(SnapKey::from_hex(bad).is_err(), "{bad}");
    }
    assert_ne!(SnapKey::generate().unwrap().id, SnapKey::generate().unwrap().id);
}

#[test]
fn the_tenant_vault_keeps_the_keys_and_a_rotation_makes_a_new_current_one_that_old_snapshots_survive() {
    use crate::cloudroles::Roles;
    use crate::cloudvaultd::{start, Client, Principal, Scope as VScope, Store};
    let vdir = crate::testdir::unique("snapkey", "vault");
    let s = Store::open(&vdir).unwrap();
    let tdir = vdir.join("tenants").join("acme");
    crate::cloudroles::save(&crate::cloudroles::path(&tdir), &Roles::new("alice").unwrap()).unwrap();
    let ak = crate::cloudkeys::MemberKeys::from_seed([1; 32]);
    s.enroll(&Principal::Member { tenant: "acme".into(), user: "alice".into() }, &ak.public()).unwrap();
    drop(s);
    let run = start("127.0.0.1:0".parse().unwrap(), &vdir, None).unwrap();
    let client = Client { host: "127.0.0.1".into(), port: run.local_addr().port(), fingerprint: Some(run.fingerprint.clone()), who: Principal::Member { tenant: "acme".into(), user: "alice".into() }, keys: ak };
    client.create_scope(&VScope::Tenant).unwrap();
    let vk = VaultKeyring { client: &client };
    assert!(vk.current().unwrap_err().contains("key-rotate"), "no key yet");
    let k1 = vk.rotate().unwrap();
    assert_eq!(vk.current().unwrap().id, k1.id);
    let (data, files) = tenant_with_noise("enc-vault");
    let snaps = data.join("cloud/snapshots");
    let m = create_with(&files, &snaps, 100, Some(&vk.current().unwrap())).unwrap();
    let k2 = vk.rotate().unwrap();
    assert_ne!(k1.id, k2.id);
    assert_eq!(vk.current().unwrap().id, k2.id);
    assert!(verify_with(&snaps, &m.id, Some(&vk)).is_ok(), "the old key is still in the vault, so the old snapshot opens");
    rekey(&snaps, &m.id, Some(&vk), &vk.current().unwrap()).unwrap();
    assert_eq!(read_manifest(&snaps, &m.id).unwrap().enc.unwrap().key_id, k2.id);
    assert!(verify_with(&snaps, &m.id, Some(&vk)).is_ok());
    // the vault holds the key as ciphertext only: the service's files do not contain it
    let all = std::fs::read_dir(vdir.join("tenants/acme/vault")).unwrap().flat_map(|e| std::fs::read(e.unwrap().path()).unwrap()).collect::<Vec<u8>>();
    assert!(!all.windows(64).any(|w| w == k1.to_hex().as_bytes()));
    run.stop();
}

#[test]
fn the_command_line_uses_the_tenant_vault_key_end_to_end_and_no_plain_archive_is_left_behind() {
    use crate::cloudroles::Roles;
    use crate::cloudvaultd::{start, Client, Principal, Scope as VScope, Store};
    let vdir = crate::testdir::unique("snapkey", "vaultcli");
    let s = Store::open(&vdir).unwrap();
    crate::cloudroles::save(&crate::cloudroles::path(&vdir.join("tenants").join("acme")), &Roles::new("alice").unwrap()).unwrap();
    let ak = crate::cloudkeys::MemberKeys::from_seed([1; 32]);
    s.enroll(&Principal::Member { tenant: "acme".into(), user: "alice".into() }, &ak.public()).unwrap();
    drop(s);
    let run = start("127.0.0.1:0".parse().unwrap(), &vdir, None).unwrap();
    let port = run.local_addr().port();
    let client = Client { host: "127.0.0.1".into(), port, fingerprint: Some(run.fingerprint.clone()), who: Principal::Member { tenant: "acme".into(), user: "alice".into() }, keys: ak.clone() };
    client.create_scope(&VScope::Tenant).unwrap();
    let (data, files) = tenant_with_noise("enc-vault-cli");
    let d = data.display().to_string();
    let vk = data.join("alice.key");
    std::fs::write(&vk, ak.seed_hex()).unwrap();
    let (vks, addr) = (vk.display().to_string(), format!("https://127.0.0.1:{port}"));
    let run_cli = |args: &[&str]| -> Result<Vec<String>, String> {
        let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        a.extend(["--data", &d, "--vault-key", &vks, "--as", "alice", "--vault", &addr, "--vault-fingerprint", &run.fingerprint, "--tenant", "acme"].map(String::from));
        let mut out = Vec::new();
        cli(&a, &mut |l| out.push(l.to_string())).map(|_| out)
    };
    // no key in the vault yet: nothing is made, in the clear or otherwise
    assert!(run_cli(&["create"]).unwrap_err().contains("key-rotate"));
    assert!(!data.join("cloud/snapshots").exists() || list(&data.join("cloud/snapshots")).is_empty());
    assert!(run_cli(&["key-rotate"]).unwrap()[0].contains("current snapshot key"));
    let said = run_cli(&["create"]).unwrap();
    assert!(said[0].contains("encrypted with key"), "{said:?}");
    let snaps = data.join("cloud/snapshots");
    let id = list(&snaps)[0].0.clone();
    // only the sealed archive and the manifests are in the folder (no tar, no zst, no staging), and the folder holds no plain contents
    let mut names: Vec<String> = std::fs::read_dir(snaps.join(&id)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    names.sort();
    assert_eq!(names, ["data.tar.zst.enc", "manifest.json", "manifest.sha256"]);
    assert!(std::fs::read_dir(&snaps).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".making")));
    assert!(run_cli(&["verify", &id]).unwrap()[0].contains("intact"));
    let r = run_cli(&["restore", &id]).unwrap();
    assert!(r[0].contains("restored 4 files"), "{r:?}");
    let to = data.join("cloud/restored").join(&id);
    assert_eq!(std::fs::read(to.join("ns/web/shared/noise.bin")).unwrap(), std::fs::read(files.join("ns/web/shared/noise.bin")).unwrap());
    assert!(std::fs::read_dir(snaps.join(&id)).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".restore")));
    // the member's key file alone, without the vault address, is refused with a reason (nothing is read or written)
    let a: Vec<String> = ["verify", &id, "--data", &d, "--vault-key", &vks, "--as", "alice"].iter().map(|s| s.to_string()).collect();
    let e = cli(&a, &mut |_| {});
    if std::env::var(crate::cloudvaultlink::ENV_URL).is_err() {
        assert!(e.unwrap_err().contains("no vault address"));
    }
    // a vault that has gone away: the sealed snapshot cannot be opened, and nothing is written
    run.stop();
    let e = run_cli(&["restore", &id, "--to", &data.join("nope").display().to_string()]).unwrap_err();
    assert!(!e.is_empty() && !data.join("nope").join("ns").exists(), "{e}");
}

#[test]
fn the_command_makes_and_uses_key_files_and_refuses_what_it_cannot_do() {
    let (data, _files) = tenant_with_noise("enc-cli");
    let d = data.display().to_string();
    let kf = data.join("snap.key");
    let kfs = kf.display().to_string();
    let run = |args: &[&str]| -> Result<Vec<String>, String> {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        cli(&a, &mut |l| out.push(l.to_string())).map(|_| out)
    };
    run(&["key-new", "--out", &kfs]).unwrap();
    assert!(run(&["key-new", "--out", &kfs]).is_err(), "an existing key file is never overwritten");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&kf).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let said = run(&["create", "--data", &d, "--key-file", &kfs]).unwrap();
    assert!(said[0].contains("encrypted with key"), "{said:?}");
    let id = list(&data.join("cloud/snapshots"))[0].0.clone();
    assert!(run(&["list", "--data", &d]).unwrap()[0].contains("ENCRYPTED"));
    assert!(run(&["verify", &id, "--data", &d, "--key-file", &kfs]).unwrap()[0].contains("intact"));
    assert!(run(&["restore", &id, "--data", &d]).unwrap_err().contains("encrypted"), "no key, no restore");
    let to = data.join("back").display().to_string();
    assert!(run(&["restore", &id, "--data", &d, "--key-file", &kfs, "--to", &to]).unwrap()[0].contains("restored 4 files"));
    assert!(run(&["rekey", &id, "--data", &d]).is_err() && run(&["key-rotate", "--data", &d]).is_err());
}
