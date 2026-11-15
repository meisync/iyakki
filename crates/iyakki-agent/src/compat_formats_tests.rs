//! Charter step 3 (docs/development/CORE-COMPATIBILITY-DESIGN.md section 10, finding F-8): golden files of two more formats, a backup
//! (`IYAKKIBK`, plain and sealed) and a cloud snapshot (`IYSNAP1` folder), recorded once by the release that made them
//! (`tests/compat/alpha3/backup-v1-*.hex`, `snapshot-v1/`) and read by every later release.
//!
//! Backward: the current code reads them and finds what was written, byte for byte after a restore. Forward: a file of a newer format, a
//! flipped byte, a cut file and a wrong passphrase are refused with nothing written. The files are synthetic (made-up names, texts and a
//! throw-away passphrase). `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha3_ -- --ignored` writes them (only when
//! the release folder is made; never to make a failing test pass).

use std::fs;
use std::path::{Path, PathBuf};

use crate::backup::{self, BackupError, CreateOptions, Layout, RestoreRequest};
use crate::cloudsnapshot as snap;

const PASSPHRASE: &str = "golden fixture passphrase, not a secret";

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-compat-fmt-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn put(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn layout(root: &Path) -> Layout {
    let l = Layout { data: root.join("data"), home: root.join("home").join("amy"), user: "amy".into() };
    fs::create_dir_all(&l.data).unwrap();
    fs::create_dir_all(&l.home).unwrap();
    l
}

/// What the recorder wrote into the backup, to compare with after a restore.
const FILES: [(&str, &str); 4] = [("layout-amy.json", "{\"tiles\":3}"), ("browser-data-amy.json", "{\"bookmarks\":[{\"url\":\"https://example.org\"}]}"), ("skills/user/amy/s.md", "a skill, golden text"), ("vault/users/amy.afv", "ALREADY-ENCRYPTED-VAULT-BYTES")];

fn fill(l: &Layout) {
    for (rel, text) in FILES {
        put(&l.data.join(rel), text);
    }
    put(&l.home.join("Dashboards/board.json"), "{\"title\":\"cpu\"}");
}

fn unhex(s: &str) -> Vec<u8> {
    let s = s.trim();
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn fixture(name: &str) -> Vec<u8> {
    unhex(&fs::read_to_string(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}")))
}

/// The fixture as a file in a new folder, under the extension the program reads.
fn as_file(root: &Path, bytes: &[u8]) -> PathBuf {
    let p = root.join(format!("golden.{}", backup::EXTENSION));
    fs::write(&p, bytes).unwrap();
    p
}

fn req(areas: &[&str]) -> RestoreRequest {
    RestoreRequest { areas: areas.iter().map(|s| s.to_string()).collect(), allow_other_account: false, now: 2_000_000 }
}

// ------------------------------------------------------------------------------------------------------------------ backup

#[test]
fn the_plain_backup_alpha3_wrote_is_read_and_restores_byte_for_byte() {
    let root = tmp("bk-plain");
    let l = layout(&root);
    let file = as_file(&root, &fixture("backup-v1-plain.hex"));
    let info = backup::inspect(&file, None, &l).unwrap();
    assert!(!info.encrypted && info.same_account && info.format == 1);
    assert_eq!((info.account.as_str(), info.created, info.total_files), ("amy", 1_000_000, 5));
    assert_eq!(info.areas.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["settings", "browser", "dashboards", "skills", "vault"]);
    backup::restore(&l, &file, None, &req(&["settings", "browser", "dashboards", "skills", "vault"])).unwrap();
    for (rel, text) in FILES {
        assert_eq!(fs::read_to_string(l.data.join(rel)).unwrap(), text, "{rel}");
    }
    assert_eq!(fs::read_to_string(l.home.join("Dashboards/board.json")).unwrap(), "{\"title\":\"cpu\"}");
    // reading it again changes nothing in the fixture
    assert_eq!(fs::read(&file).unwrap(), fixture("backup-v1-plain.hex"));
}

#[test]
fn the_sealed_backup_alpha3_wrote_opens_with_its_passphrase_and_only_with_it() {
    let root = tmp("bk-sealed");
    let l = layout(&root);
    let file = as_file(&root, &fixture("backup-v1-sealed.hex"));
    assert_eq!(backup::inspect(&file, None, &l).unwrap_err(), BackupError::NeedsPassphrase);
    assert_eq!(backup::inspect(&file, Some("wrong"), &l).unwrap_err(), BackupError::WrongPassphrase);
    let info = backup::inspect(&file, Some(PASSPHRASE), &l).unwrap();
    assert!(info.encrypted && info.total_files == 5 && info.account == "amy");
    backup::restore(&l, &file, Some(PASSPHRASE), &req(&["skills"])).unwrap();
    assert_eq!(fs::read_to_string(l.data.join("skills/user/amy/s.md")).unwrap(), "a skill, golden text");
    // nothing readable of the content sits in the file
    let raw = fs::read(&file).unwrap();
    assert!(!raw.windows(8).any(|w| w == b"golden t"), "the sealed file shows no content");
}

#[test]
fn a_newer_format_a_flipped_byte_and_a_cut_file_are_refused_and_nothing_is_written() {
    let root = tmp("bk-bad");
    let l = layout(&root);
    let good = fixture("backup-v1-plain.hex");
    // a file of a newer format: the sentence that says to update
    let mut newer = good.clone();
    newer[8..12].copy_from_slice(&(backup::FORMAT + 1).to_le_bytes());
    let e = backup::inspect(&as_file(&root, &newer), None, &l).unwrap_err();
    assert_eq!(e, BackupError::Newer { found: backup::FORMAT + 1 });
    assert!(e.to_string().contains("newer IYAKKI"));
    // a byte flipped in the file bytes, and in the middle of the manifest
    for at in [good.len() - 40, good.len() / 2, 20] {
        let mut bad = good.clone();
        bad[at] ^= 0x55;
        let r = backup::restore(&l, &as_file(&root, &bad), None, &req(&["settings", "skills"]));
        assert!(matches!(r, Err(BackupError::Damaged(_)) | Err(BackupError::NotBackup(_)) | Err(BackupError::Invalid(_)) | Err(BackupError::Unsafe(_))), "byte {at}: {r:?}");
    }
    // cut short
    let r = backup::inspect(&as_file(&root, &good[..good.len() - 7]), None, &l);
    assert!(matches!(r, Err(BackupError::Damaged(_))), "{r:?}");
    // not a backup at all
    assert!(matches!(backup::inspect(&as_file(&root, b"not a backup file at all"), None, &l), Err(BackupError::NotBackup(_))));
    assert!(!l.data.join("layout-amy.json").exists() && !l.data.join("skills").exists(), "nothing was written by a refused file");
}

// ------------------------------------------------------------------------------------------------------------------ snapshot

fn snapshot_fixture() -> (PathBuf, String) {
    let base = dir().join("snapshot-v1");
    let id = fs::read_dir(&base).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| snap::id_ok(n)).expect("a snapshot folder");
    (base, id)
}

fn copy_snapshot(root: &Path) -> (PathBuf, String) {
    let (base, id) = snapshot_fixture();
    let snaps = root.join("snaps");
    let to = snaps.join(&id);
    fs::create_dir_all(&to).unwrap();
    for e in fs::read_dir(base.join(&id)).unwrap().flatten() {
        fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
    (snaps, id)
}

const SNAP_FILES: [(&str, &str); 3] = [("notes/a.txt", "first golden note"), ("notes/b.txt", "second golden note\nwith two lines"), ("top.json", "{\"golden\":true}")];

#[test]
fn the_snapshot_alpha3_wrote_verifies_lists_and_restores_byte_for_byte() {
    let root = tmp("snap-ok");
    let (snaps, id) = copy_snapshot(&root);
    let m = snap::verify(&snaps, &id).unwrap();
    assert_eq!((m.format, m.created), (snap::FORMAT, 1_000_000));
    let files: Vec<&str> = m.items.iter().filter(|i| i.kind == "file").map(|i| i.path.as_str()).collect();
    for (rel, _) in SNAP_FILES {
        assert!(files.contains(&rel), "{rel} in {files:?}");
    }
    assert!(snap::list(&snaps).iter().any(|(i, r)| *i == id && r.is_ok()));
    let target = root.join("restored");
    snap::restore(&snaps, &id, &target, false).unwrap();
    for (rel, text) in SNAP_FILES {
        assert_eq!(fs::read_to_string(target.join(rel)).unwrap(), text, "{rel}");
    }
}

#[test]
fn a_changed_snapshot_archive_a_changed_manifest_and_a_newer_format_are_refused_and_nothing_is_unpacked() {
    let root = tmp("snap-bad");
    let (snaps, id) = copy_snapshot(&root);
    // a flipped byte in the archive
    let arch = snaps.join(&id).join("data.tar.zst");
    let good = fs::read(&arch).unwrap();
    let mut bad = good.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0x55;
    fs::write(&arch, &bad).unwrap();
    assert!(snap::verify(&snaps, &id).is_err());
    let target = root.join("restored");
    assert!(snap::restore(&snaps, &id, &target, false).is_err());
    assert!(!target.join("top.json").exists(), "a refused snapshot unpacked nothing");
    fs::write(&arch, &good).unwrap();
    assert!(snap::verify(&snaps, &id).is_ok());
    // a manifest that was changed no longer matches its checksum
    let mp = snaps.join(&id).join("manifest.json");
    let text = fs::read_to_string(&mp).unwrap();
    fs::write(&mp, text.replace("\"created\": 1000000", "\"created\": 1000001")).unwrap();
    assert!(snap::read_manifest(&snaps, &id).is_err());
    // a newer format number, with its checksum made right: read, but not understood
    let newer = text.replace(&format!("\"format\": {}", snap::FORMAT), &format!("\"format\": {}", snap::ENC_FORMAT + 7));
    fs::write(&mp, &newer).unwrap();
    let sum = <sha2::Sha256 as sha2::Digest>::digest(newer.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
    fs::write(snaps.join(&id).join("manifest.sha256"), format!("{sum}  manifest.json\n")).unwrap();
    assert!(snap::verify(&snaps, &id).is_err(), "a format this release does not know is refused");
    assert!(snap::restore(&snaps, &id, &target, false).is_err());
    assert!(!target.join("top.json").exists());
}

// ------------------------------------------------------------------------------------------------------------------ the recorders

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Writes `backup-v1-plain.hex` and `backup-v1-sealed.hex` (once, when the release is cut; never edited after).
#[test]
#[ignore = "records the golden files: IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha3_ -- --ignored"]
fn record_alpha3_backups() {
    if std::env::var("IYAKKI_COMPAT_RECORD").is_err() {
        return;
    }
    let root = tmp("rec-bk");
    let l = layout(&root);
    fill(&l);
    let dest = root.join("out");
    fs::create_dir_all(&dest).unwrap();
    let areas: std::collections::BTreeSet<String> = ["settings", "browser", "dashboards", "skills", "vault"].iter().map(|s| s.to_string()).collect();
    let plain = backup::create(&l, &dest, &CreateOptions { areas: areas.clone(), passphrase: None, now: 1_000_000, release: "0.1.0-alpha.3".into() }).unwrap();
    fs::write(dir().join("backup-v1-plain.hex"), to_hex(&fs::read(&plain.path).unwrap()) + "\n").unwrap();
    let dest2 = root.join("out2");
    fs::create_dir_all(&dest2).unwrap();
    let sealed = backup::create(&l, &dest2, &CreateOptions { areas, passphrase: Some(PASSPHRASE.into()), now: 1_000_000, release: "0.1.0-alpha.3".into() }).unwrap();
    fs::write(dir().join("backup-v1-sealed.hex"), to_hex(&fs::read(&sealed.path).unwrap()) + "\n").unwrap();
}

/// Writes `snapshot-v1/<id>/` (once, when the release is cut; never edited after).
#[test]
#[ignore = "records the golden files: IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha3_ -- --ignored"]
fn record_alpha3_snapshot() {
    if std::env::var("IYAKKI_COMPAT_RECORD").is_err() {
        return;
    }
    let root = tmp("rec-snap");
    let src = root.join("files");
    for (rel, text) in SNAP_FILES {
        put(&src.join(rel), text);
    }
    let m = snap::create(&src, &root.join("snaps"), 1_000_000).unwrap();
    let to = dir().join("snapshot-v1").join(&m.id);
    let _ = fs::remove_dir_all(dir().join("snapshot-v1"));
    fs::create_dir_all(&to).unwrap();
    for e in fs::read_dir(root.join("snaps").join(&m.id)).unwrap().flatten() {
        fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

// ------------------------------------------------------------------------------------------------------------------ the vault file

const VAULT_PASS: &str = "golden vault password, not a secret";
const VAULT_SECRETS: [(&str, &str); 2] = [("golden.token", "not-a-real-token-1"), ("golden.other", "not-a-real-token-2")];

fn vault_copy(root: &Path, text: &str) -> PathBuf {
    let p = root.join("vault.json");
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn the_vault_file_alpha3_wrote_opens_with_its_password_and_gives_back_what_was_stored() {
    use crate::vault::{LocalVault, VaultError};
    let root = tmp("vault-ok");
    let text = fs::read_to_string(dir().join("vault-v1.json")).unwrap();
    let p = vault_copy(&root, &text);
    let mut v = LocalVault::open(&p).unwrap();
    assert!(!v.is_unlocked() && v.has_slot("main") && v.slot_ids() == ["main".to_string()]);
    assert_eq!(v.unlock("main", "wrong password").unwrap_err(), VaultError::BadPassword);
    assert!(!v.is_unlocked());
    v.unlock("main", VAULT_PASS).unwrap();
    let mut names = v.names().unwrap();
    names.sort();
    assert_eq!(names, ["golden.other", "golden.token"]);
    for (k, val) in VAULT_SECRETS {
        assert_eq!(&**v.get(k).unwrap(), val);
    }
    // the current code still writes what the current code reads, on top of the old file
    v.set("golden.new", "added later").unwrap();
    let mut again = LocalVault::open(&p).unwrap();
    again.unlock("main", VAULT_PASS).unwrap();
    assert_eq!(&**again.get("golden.new").unwrap(), "added later");
    assert_eq!(&**again.get("golden.token").unwrap(), "not-a-real-token-1");
}

#[test]
fn a_newer_vault_a_flipped_byte_and_a_cut_file_are_refused_and_the_file_is_not_touched() {
    use crate::vault::{LocalVault, VaultError};
    let root = tmp("vault-bad");
    let text = fs::read_to_string(dir().join("vault-v1.json")).unwrap();
    // a newer format version
    let newer = text.replacen("\"v\": 1", "\"v\": 2", 1).replacen("\"v\":1", "\"v\":2", 1);
    assert_ne!(newer, text, "the fixture names its version");
    let p = vault_copy(&root, &newer);
    assert_eq!(LocalVault::open(&p).err(), Some(VaultError::Corrupt));
    // a cut file
    let p = vault_copy(&root, &text[..text.len() / 2]);
    assert_eq!(LocalVault::open(&p).err(), Some(VaultError::Corrupt));
    // one flipped character of the sealed data: the file opens (the slots are intact) but the data does not unlock
    let i = text.find("\"data\"").unwrap() + 12;
    let mut b = text.clone().into_bytes();
    b[i] = if b[i] == b'0' { b'1' } else { b'0' };
    let flipped = String::from_utf8(b).unwrap();
    let p = vault_copy(&root, &flipped);
    let mut v = LocalVault::open(&p).unwrap();
    assert!(v.unlock("main", VAULT_PASS).is_err());
    assert_eq!(fs::read_to_string(&p).unwrap(), flipped, "refusing changes nothing on the disk");
}

/// Writes `vault-v1.json` (once, when the release is cut; never edited after).
#[test]
#[ignore = "records the golden files: IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha3_ -- --ignored"]
fn record_alpha3_vault() {
    if std::env::var("IYAKKI_COMPAT_RECORD").is_err() {
        return;
    }
    let root = tmp("rec-vault");
    let p = root.join("vault.json");
    let mut v = crate::vault::LocalVault::create(&p, "main", VAULT_PASS).unwrap();
    for (k, val) in VAULT_SECRETS {
        v.set(k, val).unwrap();
    }
    fs::write(dir().join("vault-v1.json"), fs::read_to_string(&p).unwrap()).unwrap();
}

// ------------------------------------------------------------------------------------------------------------------ the usage ledger

#[test]
fn the_usage_ledger_lines_of_every_generation_still_load_and_write_back_unchanged() {
    use crate::usage::Entry;
    let text = fs::read_to_string(dir().join("usage-ledger-v1.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    let first: Entry = serde_json::from_str(lines[0]).unwrap();
    assert_eq!((first.h, first.user.as_str(), first.provider.as_str(), first.model.as_str(), first.input, first.output, first.calls), (490_000, "amy", "ollama", "llama3.2:3b", 1200, 340, 6));
    assert!(first.src.is_empty() && first.cat.is_empty() && first.errors == 0 && first.ms == 0 && !first.est, "the first log files had none of the later fields");
    assert_eq!(serde_json::to_string(&first).unwrap(), lines[0], "a line of the first generation is written back as it was");
    let full: Entry = serde_json::from_str(lines[1]).unwrap();
    assert_eq!((full.src.as_str(), full.cat.as_str(), full.errors, full.ms, full.est), ("app:notes", "embed", 2, 910, true));
    assert_eq!(serde_json::to_string(&full).unwrap(), lines[1]);
    let tts: Entry = serde_json::from_str(lines[2]).unwrap();
    assert_eq!((tts.src.as_str(), tts.cat.as_str()), ("mcp:calendar", "tts"));
    // a line a later release added a field to is still read, the new field ignored; a damaged line is skipped like the loader does
    let later: Entry = serde_json::from_str(lines[3]).unwrap();
    assert_eq!(later.calls, 1);
    assert!(serde_json::from_str::<Entry>("{ not json").is_err());
}
