use super::*;
use sha2::{Digest, Sha256};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-sysupd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut t = Vec::new();
    for (name, body) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        // the header checksum: the sum of all bytes with the checksum field read as spaces
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        t.extend_from_slice(&h);
        t.extend_from_slice(body);
        t.extend(std::iter::repeat_n(0u8, body.len().div_ceil(512) * 512 - body.len()));
    }
    t.extend(std::iter::repeat_n(0u8, 1024));
    t
}

fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut g = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    let chunks: Vec<&[u8]> = data.chunks(60000).collect();
    for (i, c) in chunks.iter().enumerate() {
        g.push(u8::from(i + 1 == chunks.len()));
        g.extend_from_slice(&(c.len() as u16).to_le_bytes());
        g.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        g.extend_from_slice(c);
    }
    g.extend_from_slice(&iyakki_archive::xz::crc32(data).to_le_bytes());
    g.extend_from_slice(&(data.len() as u32).to_le_bytes());
    g
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const FILES: [(&str, &str); 3] = [("boot/bzImage", "kernel bytes"), ("boot/initrd", "initrd bytes"), ("boot/grub/grub.cfg", "menuentry")];

fn release(version: &str, min_base: &str, variant: &str, tamper: bool) -> String {
    let files: Vec<Value> = FILES.iter().map(|(p, c)| serde_json::json!({"path": p, "size": c.len(), "sha256": hex(&Sha256::digest(c.as_bytes())) })).collect();
    let mut v = serde_json::json!({"schema": 1, "version": version, "variant": variant, "min_base": min_base, "notes": "Fixes the sound on laptops.", "files": files});
    if tamper {
        v["files"][0]["sha256"] = serde_json::json!("0".repeat(64));
    }
    v.to_string()
}

fn make_bundle(d: &Path, rel: &str, extra: &[(&str, &str)]) -> PathBuf {
    let mut entries: Vec<(&str, &[u8])> = vec![(RELEASE_FILE, rel.as_bytes())];
    entries.extend(FILES.iter().map(|(p, c)| (*p, c.as_bytes())));
    entries.extend(extra.iter().map(|(p, c)| (*p, c.as_bytes())));
    let p = d.join("update.tgz");
    std::fs::write(&p, gzip_stored(&tar(&entries))).unwrap();
    p
}

fn keys() -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[9u8; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

#[test]
fn a_signed_bundle_is_unpacked_checked_and_leaves_the_update_json_the_existing_flow_reads() {
    let d = tmp("ok");
    let (sk, vk) = keys();
    let b = make_bundle(&d, &release("0.2.0", "0.1.0", "pc", false), &[]);
    let sig = sign(&b, &sk).unwrap();
    let s = open(&b, &sig, &vk, &d.join("scratch"), &d.join("stage")).unwrap();
    assert_eq!((s.release.version.as_str(), s.release.notes.as_str()), ("0.2.0", "Fixes the sound on laptops."));
    assert_eq!(std::fs::read_to_string(d.join("stage/boot/bzImage")).unwrap(), "kernel bytes");
    let m: up::Manifest = serde_json::from_slice(&std::fs::read(d.join("stage/update.json")).unwrap()).unwrap();
    assert_eq!(m, s.manifest);
    up::verify_payload(&d.join("stage"), &m).unwrap();
    assert!(!d.join("scratch/archive.tar.partial").exists(), "the uncompressed tar is removed");
}

#[test]
fn a_wrong_key_a_changed_file_or_no_signature_stops_everything_before_unpacking() {
    let d = tmp("sig");
    let (sk, vk) = keys();
    let b = make_bundle(&d, &release("0.2.0", "", "pc", false), &[]);
    let sig = sign(&b, &sk).unwrap();
    let other = SigningKey::from_bytes(&[3u8; 32]).verifying_key();
    let e = open(&b, &sig, &other, &d.join("s"), &d.join("stage")).err().unwrap();
    assert!(e.contains("Nothing was unpacked"), "{e}");
    assert!(open(&b, "", &vk, &d.join("s"), &d.join("stage")).is_err());
    assert!(open(&b, "zz", &vk, &d.join("s"), &d.join("stage")).is_err());
    // one byte of the archive changed after it was signed
    let mut bytes = std::fs::read(&b).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 1;
    std::fs::write(&b, &bytes).unwrap();
    assert!(open(&b, &sig, &vk, &d.join("s"), &d.join("stage")).err().unwrap().contains("Nothing was unpacked"));
    assert!(!d.join("stage").exists(), "nothing was unpacked");
}

#[test]
fn a_signed_bundle_whose_files_do_not_match_release_json_is_refused_and_cleaned_up() {
    let d = tmp("files");
    let (sk, vk) = keys();
    // a listed file with the wrong sum
    let b = make_bundle(&d, &release("0.2.0", "", "pc", true), &[]);
    let e = open(&b, &sign(&b, &sk).unwrap(), &vk, &d.join("s"), &d.join("stage")).err().unwrap();
    assert!(e.contains("boot/bzImage"), "{e}");
    assert!(!d.join("stage").exists());
    // a file nobody listed
    let b = make_bundle(&d, &release("0.2.0", "", "pc", false), &[("boot/extra.bin", "surprise")]);
    let e = open(&b, &sign(&b, &sk).unwrap(), &vk, &d.join("s"), &d.join("stage")).err().unwrap();
    assert!(e.contains("boot/extra.bin") && e.contains("does not list"), "{e}");
    // a release.json of a newer schema is named, not guessed at
    let b = make_bundle(&d, &release("0.2.0", "", "pc", false).replace("\"schema\":1", "\"schema\":2"), &[]);
    let e = open(&b, &sign(&b, &sk).unwrap(), &vk, &d.join("s"), &d.join("stage")).err().unwrap();
    assert!(e.contains("update to a newer IYAKKI"), "{e}");
}

#[test]
fn a_bundle_is_applicable_only_on_the_same_edition_above_its_minimum_and_when_newer() {
    let d = tmp("appl");
    let (sk, vk) = keys();
    let b = make_bundle(&d, &release("0.3.0", "0.2.0", "pc", false), &[]);
    let s = open(&b, &sign(&b, &sk).unwrap(), &vk, &d.join("s"), &d.join("stage")).unwrap();
    assert!(applicable(&s, "0.2.5", "pc-x86_64").is_ok());
    assert!(applicable(&s, "0.1.0", "pc").unwrap_err().contains("needs IYAKKI 0.2.0 or newer first"));
    assert!(applicable(&s, "0.2.5", "mac").unwrap_err().contains("for the pc edition"));
    assert!(applicable(&s, "0.3.0", "pc").unwrap_err().contains("not newer"));
    assert!(applicable(&s, "0.4.0", "pc").unwrap_err().contains("not newer"));
}

#[test]
fn a_bundle_from_a_stick_is_checked_with_its_sig_file_unpacked_where_the_centre_lists_it_and_nothing_is_applied() {
    let d = tmp("centre-ok");
    let (sk, vk) = keys();
    let b = make_bundle(&d, &release("0.2.0", "0.1.0", "pc", false), &[]);
    std::fs::write(format!("{}.sig", b.display()), sign(&b, &sk).unwrap() + "\n").unwrap();
    let data = d.join("data");
    let s = open_for_centre(&data, &b, Some(&vk), "0.1.5", "pc").unwrap();
    assert_eq!(s.dir.parent().unwrap(), staged_root(&data), "unpacked under the folder the update centre lists");
    assert_eq!(std::fs::read_to_string(s.dir.join("boot/bzImage")).unwrap(), "kernel bytes");
    // the update centre's own finder lists it
    let found = crate::devmode::centre::find_packs(&staged_root(&data), Some(&vk));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].manifest.version, "0.2.0");
    assert!(!data.join("updates/scratch").exists(), "the uncompressed copy is gone");
    // opening it again replaces the staged copy instead of failing on a non-empty folder
    let again = open_for_centre(&data, &b, Some(&vk), "0.1.5", "pc").unwrap();
    assert_eq!(again.dir, s.dir);
}

#[test]
fn a_bundle_that_cannot_be_opened_is_refused_with_the_reason_and_leaves_nothing_staged() {
    let d = tmp("centre-no");
    let (sk, vk) = keys();
    let data = d.join("data");
    let b = make_bundle(&d, &release("0.2.0", "0.1.0", "pc", false), &[]);
    // no key in this build, a folder instead of a file, no .sig next to it
    assert!(open_for_centre(&data, &b, None, "0.1.5", "pc").unwrap_err().contains("no IYAKKI release key"));
    assert!(open_for_centre(&data, &d, Some(&vk), "0.1.5", "pc").unwrap_err().contains("not a file"));
    assert!(open_for_centre(&data, &b, Some(&vk), "0.1.5", "pc").unwrap_err().contains(".sig"));
    // a wrong signature (another key signed it)
    let other = SigningKey::from_bytes(&[4u8; 32]);
    std::fs::write(format!("{}.sig", b.display()), sign(&b, &other).unwrap()).unwrap();
    assert!(open_for_centre(&data, &b, Some(&vk), "0.1.5", "pc").unwrap_err().contains("Nothing was unpacked"));
    // good signature, but for another edition, too old a base, or not newer: refused and removed again
    std::fs::write(format!("{}.sig", b.display()), sign(&b, &sk).unwrap()).unwrap();
    assert!(open_for_centre(&data, &b, Some(&vk), "0.1.5", "mac").unwrap_err().contains("edition"));
    assert!(open_for_centre(&data, &b, Some(&vk), "0.0.9", "pc").unwrap_err().contains("0.1.0 or newer"));
    assert!(open_for_centre(&data, &b, Some(&vk), "0.2.0", "pc").unwrap_err().contains("not newer"));
    let left: Vec<_> = std::fs::read_dir(staged_root(&data)).map(|r| r.flatten().collect()).unwrap_or_default();
    assert!(left.is_empty(), "nothing stays staged after a refusal: {left:?}");
}

#[test]
fn the_owners_pack_makes_a_bundle_that_opens_and_refuses_a_folder_without_release_json() {
    let d = tmp("pack");
    let (sk, vk) = keys();
    let folder = d.join("folder");
    std::fs::create_dir_all(folder.join("boot/grub")).unwrap();
    for (p, c) in FILES {
        std::fs::write(folder.join(p), c).unwrap();
    }
    assert!(pack(&folder, &d.join("x.tgz")).unwrap_err().contains("release.json"));
    std::fs::write(folder.join(RELEASE_FILE), release("0.3.0", "", "pc", false)).unwrap();
    let out = d.join("update.tgz");
    pack(&folder, &out).unwrap();
    let again = d.join("again.tgz");
    pack(&folder, &again).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&again).unwrap(), "the same folder gives the same bytes");
    let s = open(&out, &sign(&out, &sk).unwrap(), &vk, &d.join("scratch"), &d.join("stage")).unwrap();
    assert_eq!(s.release.version, "0.3.0");
    assert_eq!(std::fs::read_to_string(d.join("stage/boot/grub/grub.cfg")).unwrap(), "menuentry");
}
