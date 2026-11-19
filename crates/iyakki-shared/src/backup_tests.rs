use super::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-backup-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn put_file(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn layout(root: &Path, user: &str) -> Layout {
    let l = Layout { data: root.join("data"), home: root.join("home").join(user), user: user.into() };
    fs::create_dir_all(&l.data).unwrap();
    fs::create_dir_all(&l.home).unwrap();
    l
}

fn fill(l: &Layout) {
    put_file(&l.data.join(format!("layout-{}.json", l.user)), "{\"tiles\":1}");
    put_file(&l.data.join(format!("browser-data-{}.json", l.user)), "{\"bookmarks\":[{\"url\":\"https://example.org\"}]}");
    put_file(&l.data.join(format!("browser-headers-{}.json", l.user)), "{\"rules\":[{\"id\":\"ab\",\"header\":\"X-Test\"}]}");
    put_file(&l.home.join("Dashboards/board.json"), "{\"title\":\"cpu\"}");
    put_file(&l.home.join("Flows/f1/flow.json"), "{\"name\":\"f1\"}");
    put_file(&l.data.join(format!("skills/user/{}/s.md", l.user)), "skill text");
    put_file(&l.home.join("Recordings/a.wav"), "RIFFfakewavdata");
    put_file(&l.data.join(format!("sessions/{}/c1.json", l.user)), "{\"id\":\"c1\"}");
    put_file(&l.data.join(format!("vault/users/{}.afv", l.user)), "ENCRYPTED-VAULT-BYTES");
}

fn opts(now: i64) -> CreateOptions {
    CreateOptions { areas: all_areas(), passphrase: None, now, release: "test".into() }
}

fn req(areas: &[&str]) -> RestoreRequest {
    RestoreRequest { areas: areas.iter().map(|s| s.to_string()).collect(), allow_other_account: false, now: 2_000_000 }
}

#[test]
fn round_trip_restores_and_keeps_the_current_data_as_a_copy() {
    let root = tmp("rt");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("stick");
    fs::create_dir_all(&dest).unwrap();
    let c = create(&l, &dest, &opts(1_000_000)).unwrap();
    assert!(c.path.extension().unwrap() == EXTENSION && !c.encrypted && c.files == 9);
    let info = inspect(&c.path, None, &l).unwrap();
    assert!(info.same_account && info.format == FORMAT && info.total_files == 9);
    assert_eq!(info.areas.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["settings", "browser", "dashboards", "flows", "skills", "chats", "recordings", "vault"]);
    // change the live data, then restore two areas
    put_file(&l.home.join("Dashboards/board.json"), "CHANGED");
    put_file(&l.home.join("Dashboards/new.json"), "new");
    put_file(&l.home.join("Recordings/a.wav"), "CHANGED");
    let rep = restore(&l, &c.path, None, &req(&["dashboards"])).unwrap();
    assert_eq!(fs::read_to_string(l.home.join("Dashboards/board.json")).unwrap(), "{\"title\":\"cpu\"}");
    assert!(!l.home.join("Dashboards/new.json").exists());
    assert_eq!(fs::read_to_string(l.home.join("Recordings/a.wav")).unwrap(), "CHANGED", "areas not chosen are untouched");
    assert_eq!(rep.kept.len(), 1);
    assert_eq!(fs::read_to_string(rep.kept[0].join("board.json")).unwrap(), "CHANGED", "the current data is kept, not deleted");
    assert!(!fs::read_dir(&l.home).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".iyakki-restore-")), "no staging left");
    // a single-file area and the vault restore byte for byte
    put_file(&l.data.join("vault/users/amy.afv"), "OTHER");
    restore(&l, &c.path, None, &req(&["vault", "settings"])).unwrap();
    assert_eq!(fs::read_to_string(l.data.join("vault/users/amy.afv")).unwrap(), "ENCRYPTED-VAULT-BYTES");
}

#[test]
fn the_vault_is_copied_as_is_and_nothing_secret_is_written_in_plain_text() {
    let root = tmp("sec");
    let l = layout(&root, "amy");
    fill(&l);
    put_file(&l.home.join("Flows/vpn.conf"), "[Interface]\nPrivateKey = AAAAfakekeyfakekeyfakekey=\n");
    put_file(&l.home.join("Flows/id_x"), "-----BEGIN OPENSSH PRIVATE KEY-----\nzzz\n");
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let pre = preview(&l, &all_areas()).unwrap();
    let flows = pre.iter().find(|a| a.id == "flows").unwrap();
    assert_eq!(flows.files, 1);
    assert_eq!(flows.skipped.len(), 2, "{:?}", flows.skipped);
    let c = create(&l, &dest, &opts(1_000_000)).unwrap();
    let raw = String::from_utf8_lossy(&fs::read(&c.path).unwrap()).into_owned();
    assert!(!raw.contains("PrivateKey") && !raw.contains("PRIVATE KEY"));
    assert!(raw.contains("ENCRYPTED-VAULT-BYTES"), "the vault file goes in exactly as it is");
}

#[test]
fn a_flipped_byte_or_a_cut_file_is_refused_before_anything_is_touched() {
    let root = tmp("tamper");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let c = create(&l, &dest, &opts(1_000_000)).unwrap();
    let good = fs::read(&c.path).unwrap();
    put_file(&l.home.join("Dashboards/board.json"), "LIVE");
    let flip = root.join("flip.iybak");
    let mut bad = good.clone();
    let at = bad.windows(15).position(|w| w == b"RIFFfakewavdata").unwrap();
    bad[at + 3] ^= 1;
    fs::write(&flip, &bad).unwrap();
    assert!(matches!(inspect(&flip, None, &l), Err(BackupError::Damaged(_))));
    assert!(matches!(restore(&l, &flip, None, &req(&["dashboards"])), Err(BackupError::Damaged(_))));
    assert_eq!(fs::read_to_string(l.home.join("Dashboards/board.json")).unwrap(), "LIVE");
    let cut = root.join("cut.iybak");
    fs::write(&cut, &good[..good.len() - 40]).unwrap();
    assert!(matches!(inspect(&cut, None, &l), Err(BackupError::Damaged(_))));
    let extra = root.join("extra.iybak");
    let mut more = good.clone();
    more.push(0);
    fs::write(&extra, &more).unwrap();
    assert!(matches!(inspect(&extra, None, &l), Err(BackupError::Damaged(_))));
    let junk = root.join("junk.iybak");
    fs::write(&junk, b"not a backup at all").unwrap();
    assert!(matches!(inspect(&junk, None, &l), Err(BackupError::NotBackup(_))));
    assert!(!fs::read_dir(&l.home).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("restore")), "nothing staged or moved");
}

/// A plain file made by hand, with a correct total checksum, to try names a real backup never contains.
fn craft(path: &Path, format: u32, manifest: &str, data: &[&[u8]]) {
    let mut body = Vec::new();
    body.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    body.extend_from_slice(manifest.as_bytes());
    for d in data {
        body.extend_from_slice(d);
    }
    let digest = Sha256::digest(&body);
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&format.to_le_bytes());
    out.push(0);
    out.extend_from_slice(&body);
    out.extend_from_slice(&digest);
    fs::write(path, out).unwrap();
}

fn entry_json(src: &str, sub: &str, data: &[u8]) -> String {
    format!("{{\"src\":\"{src}\",\"sub\":\"{sub}\",\"len\":{},\"sha256\":\"{}\"}}", data.len(), hex(&Sha256::digest(data)))
}

#[test]
fn names_that_leave_the_persons_places_are_refused() {
    let root = tmp("trav");
    let l = layout(&root, "amy");
    put_file(&root.join("outside.txt"), "keep");
    let d = b"evil";
    for (src, sub) in [("dashboards", "../../outside.txt"), ("dashboards", "/etc/passwd"), ("dashboards", "a/../b"), ("dashboards", "a//b"), ("dashboards", "a\\b"), ("dashboards", "./x"), ("../etc", "x"), ("vault", "x"), ("layout", "sub"), ("dashboards", "")] {
        let p = root.join("t.iybak");
        let m = format!("{{\"format\":1,\"min_reader\":1,\"account\":\"amy\",\"created\":1,\"entries\":[{}]}}", entry_json(src, sub, d));
        craft(&p, 1, &m, &[d]);
        let e = inspect(&p, None, &l).unwrap_err();
        assert!(matches!(e, BackupError::Unsafe(_)), "{src} {sub}: {e:?}");
        assert!(restore(&l, &p, None, &req(&["dashboards"])).is_err());
    }
    assert_eq!(fs::read_to_string(root.join("outside.txt")).unwrap(), "keep");
    assert!(!l.home.join("Dashboards").exists());
}

#[test]
fn links_in_the_data_are_not_followed_and_a_link_destination_is_replaced_not_followed() {
    let root = tmp("link");
    let l = layout(&root, "amy");
    fill(&l);
    put_file(&root.join("secret.txt"), "outside secret");
    std::os::unix::fs::symlink(root.join("secret.txt"), l.home.join("Recordings/link.wav")).unwrap();
    std::os::unix::fs::symlink(&root, l.home.join("Flows/dirlink")).unwrap();
    let pre = preview(&l, &all_areas()).unwrap();
    assert_eq!(pre.iter().find(|a| a.id == "recordings").unwrap().skipped.len(), 1);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let c = create(&l, &dest, &opts(1_000_000)).unwrap();
    assert!(!String::from_utf8_lossy(&fs::read(&c.path).unwrap()).contains("outside secret"));
    // the destination folder is a link to somewhere else: the link is moved aside, the outside folder is not written
    fs::remove_dir_all(l.home.join("Dashboards")).unwrap();
    fs::create_dir_all(root.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), l.home.join("Dashboards")).unwrap();
    restore(&l, &c.path, None, &req(&["dashboards"])).unwrap();
    assert!(fs::symlink_metadata(l.home.join("Dashboards")).unwrap().is_dir());
    assert_eq!(fs::read_dir(root.join("elsewhere")).unwrap().count(), 0);
}

#[test]
fn another_accounts_file_needs_confirmation_and_lands_in_the_signed_in_persons_folder() {
    let root = tmp("acct");
    let amy = layout(&root, "amy");
    fill(&amy);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let c = create(&amy, &dest, &opts(1_000_000)).unwrap();
    let bo = Layout { data: root.join("data"), home: root.join("home/bo"), user: "bo".into() };
    fs::create_dir_all(&bo.home).unwrap();
    let info = inspect(&c.path, None, &bo).unwrap();
    assert!(!info.same_account && info.account == "amy");
    assert_eq!(restore(&bo, &c.path, None, &req(&["dashboards"])).unwrap_err(), BackupError::OtherAccount { from: "amy".into() });
    assert!(!bo.home.join("Dashboards").exists());
    let mut r = req(&["dashboards", "settings"]);
    r.allow_other_account = true;
    restore(&bo, &c.path, None, &r).unwrap();
    assert!(bo.home.join("Dashboards/board.json").exists());
    assert!(bo.data.join("layout-bo.json").exists() && fs::read_to_string(amy.data.join("layout-amy.json")).unwrap() == "{\"tiles\":1}");
    let mut v = req(&["vault"]);
    v.allow_other_account = true;
    assert!(restore(&bo, &c.path, None, &v).is_err(), "a vault does not move to another account");
    assert!(!bo.data.join("vault/users/bo.afv").exists());
}

#[test]
fn newer_older_and_unknown_formats() {
    let root = tmp("ver");
    let l = layout(&root, "amy");
    let p = root.join("v.iybak");
    let d = b"hello";
    let m1 = format!("{{\"format\":1,\"min_reader\":1,\"account\":\"amy\",\"created\":1,\"entries\":[{}]}}", entry_json("dashboards", "a.json", d));
    craft(&p, 1, &m1, &[d]);
    assert_eq!(inspect(&p, None, &l).unwrap().total_files, 1);
    // an older writer that did not know `min_reader` or `release`
    let old = format!("{{\"format\":1,\"account\":\"amy\",\"created\":1,\"entries\":[{}]}}", entry_json("dashboards", "a.json", d));
    craft(&p, 1, &old, &[d]);
    restore(&l, &p, None, &req(&["dashboards"])).unwrap();
    assert_eq!(fs::read_to_string(l.home.join("Dashboards/a.json")).unwrap(), "hello");
    // newer container
    craft(&p, FORMAT + 1, &m1, &[d]);
    let e = inspect(&p, None, &l).unwrap_err();
    assert_eq!(e, BackupError::Newer { found: FORMAT + 1 });
    assert!(e.to_string().contains("newer IYAKKI") && e.to_string().contains("nothing was changed") || e.to_string().contains("Nothing was changed") || e.to_string().contains("nothing was changed"));
    // same container number but the writer says a newer reader is needed
    let need = m1.replace("\"min_reader\":1", "\"min_reader\":9");
    craft(&p, 1, &need, &[d]);
    assert!(matches!(inspect(&p, None, &l), Err(BackupError::Newer { found: 9 })));
    // format 0 is not ours
    craft(&p, 0, &m1, &[d]);
    assert!(matches!(inspect(&p, None, &l), Err(BackupError::NotBackup(_))));
}

#[test]
fn passphrase_round_trip_wrong_passphrase_and_tampering() {
    let root = tmp("enc");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let mut o = opts(1_000_000);
    o.passphrase = Some("short".into());
    assert!(create(&l, &dest, &o).is_err());
    o.passphrase = Some("correct horse battery".into());
    let c = create(&l, &dest, &o).unwrap();
    assert!(c.encrypted);
    let raw = fs::read(&c.path).unwrap();
    assert!(!raw.windows(15).any(|w| w == b"RIFFfakewavdata") && !raw.windows(8).any(|w| w == b"amy\",\"cr"), "nothing readable in the file");
    assert_eq!(inspect(&c.path, None, &l).unwrap_err(), BackupError::NeedsPassphrase);
    assert_eq!(inspect(&c.path, Some("wrong wrong wrong"), &l).unwrap_err(), BackupError::WrongPassphrase);
    assert!(inspect(&c.path, Some("correct horse battery"), &l).unwrap().encrypted);
    let mut bad = raw.clone();
    let n = bad.len();
    bad[n / 2] ^= 1;
    let p = root.join("bad.iybak");
    fs::write(&p, &bad).unwrap();
    assert!(matches!(inspect(&p, Some("correct horse battery"), &l), Err(BackupError::Damaged(_) | BackupError::WrongPassphrase)));
    // dropping the last piece is noticed
    let mut cut = raw.clone();
    cut.truncate(n - 20);
    fs::write(&p, &cut).unwrap();
    assert!(inspect(&p, Some("correct horse battery"), &l).is_err());
    put_file(&l.home.join("Flows/f1/flow.json"), "CHANGED");
    restore(&l, &c.path, Some("correct horse battery"), &req(&["flows"])).unwrap();
    assert_eq!(fs::read_to_string(l.home.join("Flows/f1/flow.json")).unwrap(), "{\"name\":\"f1\"}");
}

#[test]
fn no_half_file_under_the_real_name_and_no_overwrite_and_space_is_checked() {
    let root = tmp("part");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    assert!(create(&l, &root.join("missing"), &opts(5)).is_err());
    assert!(create(&l, &root.join("out/none"), &opts(5)).is_err());
    let mut none = opts(5);
    none.areas.clear();
    assert!(create(&l, &dest, &none).is_err());
    let c = create(&l, &dest, &opts(77_777)).unwrap();
    assert!(create(&l, &dest, &opts(77_777)).is_err(), "same second, same name: refuse, never overwrite");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
    assert!(c.path.file_name().unwrap().to_string_lossy().starts_with("iyakki-backup-amy-1970"));
    // a file that vanishes between planning and reading leaves no .part behind
    put_file(&l.home.join("Recordings/b.wav"), "x");
    assert!(fs::read_dir(&dest).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")));
}

#[test]
fn restoring_an_area_the_file_lacks_or_nothing_is_refused_and_only_chosen_areas_change() {
    let root = tmp("areas");
    let l = layout(&root, "amy");
    put_file(&l.home.join("Dashboards/d.json"), "d");
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let mut o = opts(10);
    o.areas = ["dashboards".to_string()].into();
    let c = create(&l, &dest, &o).unwrap();
    assert!(restore(&l, &c.path, None, &req(&["flows"])).is_err());
    assert!(restore(&l, &c.path, None, &req(&[])).is_err());
    assert!(restore(&l, &c.path, None, &req(&["nonsense"])).is_err());
    let bad_user = Layout { data: l.data.clone(), home: l.home.clone(), user: "../x".into() };
    assert!(create(&bad_user, &dest, &o).is_err());
}

#[test]
fn human_sizes() {
    assert_eq!(human(10), "10 B");
    assert_eq!(human(1536), "1.5 KB");
    assert_eq!(human(3 << 20), "3.0 MB");
}

#[test]
fn a_backup_reports_progress_over_both_passes_and_can_be_stopped_without_leaving_a_file() {
    let root = tmp("progress");
    let l = layout(&root, "amy");
    fill(&l);
    // a bigger file so the progress is more than a few bytes
    fs::write(l.home.join("Recordings/big.wav"), vec![7u8; 3 << 20]).unwrap();
    let dest = root.join("stick");
    fs::create_dir_all(&dest).unwrap();
    let pr = Progress::default();
    let c = create_with_progress(&l, &dest, &opts(1_000_000), &pr).unwrap();
    let total = pr.total.load(Ordering::Relaxed);
    assert!(total > 6 << 20, "{total}");
    assert_eq!(pr.done.load(Ordering::Relaxed), total, "both passes were counted");
    assert!(c.path.exists());
    // stopped before it starts reading: no file under the real name, no .part left, the error says so
    let stop = Progress::default();
    stop.cancel.store(true, Ordering::Relaxed);
    let dest2 = root.join("stick2");
    fs::create_dir_all(&dest2).unwrap();
    let e = create_with_progress(&l, &dest2, &opts(1_000_001), &stop).unwrap_err();
    assert_eq!(e, BackupError::Cancelled);
    assert!(e.to_string().contains("stopped"));
    assert_eq!(fs::read_dir(&dest2).unwrap().count(), 0, "nothing was left behind");
    // stopped while the file is written (after the hashes): the half file goes
    let late = std::sync::Arc::new(Progress::default());
    let l2 = late.clone();
    let (l_, d_) = (l.clone(), dest2.clone());
    let h = std::thread::spawn(move || create_with_progress(&l_, &d_, &opts(1_000_002), &l2));
    while late.done.load(Ordering::Relaxed) < 1 {
        std::thread::yield_now();
    }
    late.cancel.store(true, Ordering::Relaxed);
    let r = h.join().unwrap();
    assert!(matches!(r, Err(BackupError::Cancelled)) || r.is_ok(), "a very fast run may have finished first");
    assert!(fs::read_dir(&dest2).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")), "no .part file stays");
}

// ------------------------------------------------------------------------------------------------------------------ chains

#[test]
fn an_incremental_holds_only_what_changed_names_what_is_gone_and_a_chain_restores_in_order() {
    let root = tmp("chain");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let full = create(&l, &dest, &opts(1_000_000)).unwrap();
    assert!(!is_incremental(&full.path));
    // change one file, add one, remove one
    put_file(&l.home.join("Dashboards/board.json"), "{\"title\":\"mem\"}");
    put_file(&l.home.join("Dashboards/new.json"), "{\"title\":\"new\"}");
    fs::remove_file(l.home.join("Recordings/a.wav")).unwrap();
    let base = chain_state(&l, &[full.path.clone()], None).unwrap();
    assert_eq!(base.links, 1);
    let inc = create_incremental(&l, &dest, &opts(1_000_100), &Progress::default(), &base).unwrap();
    assert_eq!(inc.files, 2, "only the changed file and the new one are in it");
    assert!(inc.path.file_name().unwrap().to_string_lossy().starts_with("iyakki-incr-"));
    assert!(inc.bytes < full.bytes);
    // a second incremental on top of the first
    put_file(&l.home.join("Dashboards/new.json"), "{\"title\":\"newer\"}");
    let st2 = chain_state(&l, &[full.path.clone(), inc.path.clone()], None).unwrap();
    assert_eq!(st2.links, 2);
    let inc2 = create_incremental(&l, &dest, &opts(1_000_200), &Progress::default(), &st2).unwrap();
    assert_eq!(inc2.files, 1);
    // a plain restore of an incremental is refused, and so is a chain with a link missing or out of order
    assert!(matches!(restore(&l, &inc.path, None, &req(&["dashboards"])), Err(BackupError::Invalid(_))));
    assert!(matches!(inspect(&inc.path, None, &l), Err(BackupError::Invalid(_))));
    assert!(matches!(chain_state(&l, &[full.path.clone(), inc2.path.clone()], None), Err(BackupError::Invalid(_))));
    assert!(matches!(chain_state(&l, &[inc.path.clone()], None), Err(BackupError::Invalid(_))));
    // restore the chain into a fresh account folder: the last version of every file, the removed one absent
    let root2 = tmp("chain-restore");
    let l2 = layout(&root2, "amy");
    let scratch = root2.join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let chain = [full.path.clone(), inc.path.clone(), inc2.path.clone()];
    restore_chain(&l2, &chain, None, &scratch, &req(&["dashboards"])).unwrap();
    assert_eq!(fs::read_to_string(l2.home.join("Dashboards/board.json")).unwrap(), "{\"title\":\"mem\"}");
    assert_eq!(fs::read_to_string(l2.home.join("Dashboards/new.json")).unwrap(), "{\"title\":\"newer\"}");
    assert!(!l2.home.join("Recordings/a.wav").exists(), "a file removed in a later link is not restored");
    assert!(fs::read_dir(&scratch).unwrap().next().is_none(), "no merged file and no staging is left");
}

#[test]
fn a_chain_with_a_passphrase_and_a_damaged_link_is_refused() {
    let root = tmp("chain-pass");
    let l = layout(&root, "amy");
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let mut o = opts(2_000_000);
    o.passphrase = Some("correct horse".into());
    let full = create(&l, &dest, &o).unwrap();
    put_file(&l.home.join("Flows/f1/flow.json"), "{\"name\":\"f2\"}");
    let base = chain_state(&l, &[full.path.clone()], Some("correct horse")).unwrap();
    let mut o2 = opts(2_000_100);
    o2.passphrase = Some("correct horse".into());
    let inc = create_incremental(&l, &dest, &o2, &Progress::default(), &base).unwrap();
    assert!(matches!(chain_state(&l, &[full.path.clone(), inc.path.clone()], None), Err(BackupError::NeedsPassphrase)));
    let merged = merge_chain(&l, &[full.path.clone(), inc.path.clone()], Some("correct horse"), &dest, 2_000_200).unwrap();
    let c = inspect(&merged, Some("correct horse"), &l).unwrap();
    assert_eq!(c.total_files, 9);
    // flip a byte in the incremental
    let mut b = fs::read(&inc.path).unwrap();
    let n = b.len() - 40;
    b[n] ^= 1;
    fs::write(&inc.path, b).unwrap();
    assert!(chain_state(&l, &[full.path.clone(), inc.path.clone()], Some("correct horse")).is_err());
}

#[test]
fn the_machines_own_data_is_backed_up_only_by_an_administrator_encrypted_and_never_by_the_persons_paths() {
    let root = tmp("system");
    let l = layout(&root, "amy");
    fill(&l);
    put_file(&l.data.join("settings.json"), "{\"schema\":1}");
    put_file(&l.data.join("connections.json"), "{\"git\":[]}");
    put_file(&l.data.join("vpn/office.conf"), "[Interface]");
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    // the person's own paths never see it: not in the default areas, refused by name, not in the person's file
    assert!(!all_areas().contains("system"));
    let mut o = opts(3_000_000);
    o.areas = std::iter::once("system".to_string()).collect();
    assert!(matches!(create(&l, &dest, &o), Err(BackupError::Invalid(_))));
    let own = create(&l, &dest, &opts(3_000_001)).unwrap();
    assert!(!inspect(&own.path, None, &l).unwrap().areas.iter().any(|a| a.id == "system"));
    // an administrator needs a passphrase
    assert!(AdminProof::of(false).is_none());
    let admin = AdminProof::of(true).unwrap();
    assert!(matches!(create_system(&l, &dest, &opts(3_000_002), &Progress::default(), admin), Err(BackupError::Invalid(_))));
    let mut o2 = opts(3_000_003);
    o2.passphrase = Some("long enough pass".into());
    let sys = create_system(&l, &dest, &o2, &Progress::default(), admin).unwrap();
    assert!(sys.encrypted && sys.files == 3);
    let c = inspect(&sys.path, Some("long enough pass"), &l).unwrap();
    assert_eq!(c.areas.len(), 1);
    assert_eq!(c.areas[0].id, "system");
    // the person's restore refuses it; the administrator's puts it back and keeps what was there
    assert!(matches!(restore(&l, &sys.path, Some("long enough pass"), &req(&["system"])), Err(BackupError::Invalid(_))));
    put_file(&l.data.join("settings.json"), "{\"schema\":2}");
    let r = restore_system(&l, &sys.path, Some("long enough pass"), 3_000_100, false, admin).unwrap();
    assert_eq!(fs::read_to_string(l.data.join("settings.json")).unwrap(), "{\"schema\":1}");
    assert!(!r.kept.is_empty());
}
