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
