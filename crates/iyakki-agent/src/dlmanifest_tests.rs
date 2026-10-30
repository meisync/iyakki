use super::*;
use iyakki_proxy::{StreamHead, Streamer};

/// A throwaway key for tests only (a fixed seed in this file). It is not, and must never be, the owner's key.
pub(super) fn test_signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

fn other_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-dlm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn signed(m: &Manifest) -> (Vec<u8>, String) {
    let bytes = to_json(m).into_bytes();
    let sig = sign_bytes(&test_signing_key(), &bytes);
    (bytes, sig)
}

fn newer(f: impl FnOnce(&mut Manifest)) -> Manifest {
    let mut m = bundled();
    m.sequence += 1;
    m.issued = "2026-11-01".into();
    f(&mut m);
    m
}

fn item<'a>(m: &'a mut Manifest, id: &str) -> &'a mut Item {
    m.items.iter_mut().find(|i| i.id == id).unwrap()
}

fn store(name: &str) -> Store {
    Store::new(&dir(name))
}

fn refused(s: &Store, m: &Manifest) -> String {
    let (b, g) = signed(m);
    s.apply(&b, &g).unwrap_err()
}

// ---- the bundled data

#[test]
fn the_bundled_manifest_is_valid_and_has_no_secret_key() {
    let b = bundled();
    validate(&b).unwrap();
    assert!(b.items.iter().all(|i| i.downloaded_not_shipped));
    assert!(!BUNDLED.contains("PRIVATE") && key_in(PUBKEY_FILE).is_none_or(|k| k.as_bytes().len() == 32));
    // the key file is either `unset` or a public key, and says never to put a private key in it
    assert!(PUBKEY_FILE.contains("Never put a private key here"));
}

#[test]
fn the_key_file_is_unset_until_the_owner_makes_one() {
    assert!(key_in("# comment\nunset\n").is_none());
    assert!(key_in(&format!("# c\n{}\n", crate::pkg::public_key_hex(&other_key()))).is_some());
    assert!(key_in("zz").is_none());
}

#[test]
fn every_review_row_of_the_download_list_is_covered_with_the_same_kind_licence_and_pin() {
    let rows = download_rows(&std::fs::read_to_string(repo().join("legal/downloads.txt")).unwrap());
    let b = bundled();
    let review: Vec<_> = rows.iter().filter(|r| is_review(&r.2)).collect();
    assert_eq!(review.len(), 36, "the licence gate lists 36 REVIEW rows");
    for r in review {
        let it = b.items.iter().find(|i| i.id == r.0 || i.covers.contains(&r.0)).unwrap_or_else(|| panic!("REVIEW row {} is not in legal/downloads-manifest.json", r.0));
        assert_eq!(it.licence, r.1, "{}: licence differs from legal/downloads.txt", r.0);
        match r.2.as_str() {
            "checksum" => assert!(it.kind == Kind::FloatSum && matches!(it.checksum, Some(Checksum::VendorFile { .. })), "{}: a checksum row needs a vendor checksum file in the manifest", r.0),
            "none-vendor" => assert!(it.kind == Kind::NoneVendor && it.version == r.3, "{}: the pinned version must be the row's ({})", r.0, r.3),
            _ => assert_eq!(it.kind, Kind::NoPin, "{}", r.0),
        }
    }
}

#[test]
fn the_bundled_file_is_what_the_code_and_the_download_list_say() {
    let rows = download_rows(&std::fs::read_to_string(repo().join("legal/downloads.txt")).unwrap());
    let b = bundled();
    let made = from_catalog(&rows, b.sequence, &b.issued);
    assert_eq!(made, b, "legal/downloads-manifest.json drifted from clis.rs and legal/downloads.txt: run the ignored test `regenerate_the_bundled_file`");
}

#[test]
#[ignore = "writes legal/downloads-manifest.json from the code; run by hand after a change to clis.rs or legal/downloads.txt"]
fn regenerate_the_bundled_file() {
    let rows = download_rows(&std::fs::read_to_string(repo().join("legal/downloads.txt")).unwrap());
    let seq = std::fs::read_to_string(repo().join("legal/downloads-manifest.json")).ok().and_then(|t| parse(t.as_bytes()).ok());
    let (sequence, issued) = seq.map_or((1, "2026-10-28".to_string()), |m| (m.sequence, m.issued));
    std::fs::write(repo().join("legal/downloads-manifest.json"), to_json(&from_catalog(&rows, sequence, &issued))).unwrap();
}

#[test]
fn proprietary_items_are_flagged_and_nothing_is_shipped() {
    let b = bundled();
    for id in PROPRIETARY {
        assert!(b.items.iter().find(|i| i.id == *id).unwrap().proprietary, "{id}");
    }
    assert!(b.items.iter().filter(|i| i.proprietary).all(|i| PROPRIETARY.contains(&i.id.as_str())));
}

// ---- signature

#[test]
fn a_good_manifest_is_accepted_saved_and_reported() {
    let s = store("good");
    let m = newer(|m| {
        item(m, "cli:kubectl").version = "v1.99.0".into();
    });
    let (b, g) = signed(&m);
    let r = s.apply(&b, &g).unwrap();
    assert_eq!(r, Applied::Updated { sequence: 2, changed: vec!["cli:kubectl".into()] });
    assert_eq!(s.active().items.iter().find(|i| i.id == "cli:kubectl").unwrap().version, "v1.99.0");
    assert_eq!(s.apply(&b, &g).unwrap(), Applied::Unchanged);
    // a new Store on the same folder (a restart) still uses it
    assert_eq!(Store::new(&s.dir).active().sequence, 2);
}

#[test]
fn unsigned_wrongly_signed_tampered_and_malformed_manifests_are_refused() {
    let s = store("sig");
    let m = newer(|m| item(m, "cli:kubectl").version = "v1.99.0".into());
    let (b, g) = signed(&m);
    assert!(s.apply(&b, "").unwrap_err().contains("128"));
    assert!(s.apply(&b, "zz").unwrap_err().contains("128"));
    let wrong = sign_bytes(&other_key(), &b);
    assert!(s.apply(&b, &wrong).unwrap_err().contains("does not match"));
    let mut tampered = b.clone();
    let at = String::from_utf8_lossy(&b).find("v1.99.0").unwrap();
    tampered[at + 3] = b'8';
    assert!(s.apply(&tampered, &g).unwrap_err().contains("does not match"));
    let junk = b"{not json";
    assert!(s.apply(junk, &sign_bytes(&test_signing_key(), junk)).unwrap_err().contains("malformed"));
    let unknown = to_json(&m).replace("\"schema\"", "\"extra\": 1, \"schema\"").into_bytes();
    assert!(s.apply(&unknown, &sign_bytes(&test_signing_key(), &unknown)).unwrap_err().contains("malformed"));
    assert!(!s.file().exists(), "nothing was kept");
    assert_eq!(s.active(), bundled());
}

#[test]
fn without_a_built_in_key_nothing_fetched_is_accepted() {
    let s = Store::with(&dir("nokey"), None, bundled());
    let (b, g) = signed(&newer(|_| {}));
    assert!(s.apply(&b, &g).unwrap_err().contains("no manifest key"));
}

#[test]
fn a_too_large_manifest_is_refused() {
    let s = store("big");
    let big = vec![b' '; MAX_BYTES + 1];
    assert!(s.apply(&big, &sign_bytes(&test_signing_key(), &big)).unwrap_err().contains("too large"));
}

// ---- the rules: only version, url and checksum of existing items

#[test]
fn a_manifest_cannot_add_an_item_change_a_licence_or_a_flag_or_a_kind() {
    let s = store("rules");
    let mut add = newer(|_| {});
    let mut extra = add.items[0].clone();
    extra.id = "cli:new-thing".into();
    extra.covers.clear();
    add.items.push(extra);
    assert!(refused(&s, &add).contains("cannot add an item"));
    for f in [
        (|i: &mut Item| i.licence = "MIT".into()) as fn(&mut Item),
        |i| i.proprietary = !i.proprietary,
        |i| i.downloaded_not_shipped = false,
        |i| i.kind = Kind::NoPin,
        |i| i.covers.push("zzz".into()),
        |i| i.hosts.push("evil.example".into()),
    ] {
        let m = newer(|m| f(item(m, "cli:kubectl")));
        assert!({ let e = refused(&s, &m); e.contains("fixed by this program") || e.contains("not a plain") || e.contains("must be true") || e.contains("does not go with") }, "{}", refused(&s, &m));
    }
    // a proprietary item stays as the program has it
    let m = newer(|m| item(m, "cli:claude-code").proprietary = false);
    assert!(refused(&s, &m).contains("cannot be changed"));
    assert_eq!(s.active(), bundled());
}

#[test]
fn addresses_must_be_https_on_the_items_own_hosts() {
    let s = store("hosts");
    for bad in [
        "http://dl.k8s.io/release/{v}/kubectl",
        "https://evil.example/kubectl",
        "https://dl.k8s.io.evil.example/kubectl",
        "https://k8s.io@evil.example/kubectl",
        "https://dl.k8s.io:8443/kubectl",
        "https://dl.k8s.io\\@evil.example/kubectl",
        "https://{v}.evil.example/kubectl",
        "https://dl.k8s.io/a b",
        "file:///etc/passwd",
    ] {
        let m = newer(|m| item(m, "cli:kubectl").url = bad.into());
        assert!(refused(&s, &m).contains("cli:kubectl"), "{bad} was not refused");
    }
    let m = newer(|m| {
        if let Some(Checksum::VendorFile { url, .. }) = &mut item(m, "cli:kubectl").checksum {
            *url = "https://evil.example/kubectl.sha256".into();
        }
    });
    assert!(refused(&s, &m).contains("checksum file"));
    // an item that has no address in the program cannot be given one
    let m = newer(|m| item(m, "cli:aider").url = "https://pypi.org/x".into());
    assert!(refused(&s, &m).contains("cannot add or remove"));
    // a sub-domain of an allowed host is fine
    let m = newer(|m| item(m, "cli:kubectl").url = "https://cdn.dl.k8s.io/release/{v}/bin/linux/{arch}/kubectl".into());
    assert!(matches!(s.apply(&signed(&m).0, &signed(&m).1), Ok(Applied::Updated { .. })));
}

#[test]
fn the_checksum_may_change_value_and_address_but_not_kind() {
    let s = store("sums");
    // the vendor's checksum file is never replaced by our own value
    let m = newer(|m| item(m, "cli:kubectl").checksum = Some(Checksum::Sha256 { files: vec![FileSum { file: "kubectl".into(), sha256: "a".repeat(64) }], date: "2026-11-01".into(), source: "test".into() }));
    assert!(refused(&s, &m).contains("not its kind") || refused(&s, &m).contains("does not go with"));
    // and cannot be removed
    let m = newer(|m| item(m, "cli:kubectl").checksum = None);
    assert!(refused(&s, &m).contains("does not go with"));
    // an item with no vendor checksum may get our own, with date and source
    let good = |sha: String, date: &str| newer(|m| {
        let i = item(m, "cli:aider");
        i.version = "0.87.0".into();
        i.checksum = Some(Checksum::Sha256 { files: vec![FileSum { file: "aider.whl".into(), sha256: sha }], date: date.into(), source: "PyPI file list read over HTTPS".into() });
    });
    assert!(refused(&s, &good("A".repeat(64), "2026-11-01")).contains("sha256"));
    assert!(refused(&s, &good("a".repeat(63), "2026-11-01")).contains("sha256"));
    assert!(refused(&s, &good("a".repeat(64), "yesterday")).contains("sha256"));
    let ok = good("a".repeat(64), "2026-11-01");
    assert!(matches!(s.apply(&signed(&ok).0, &signed(&ok).1), Ok(Applied::Updated { changed, .. }) if changed == vec!["cli:aider".to_string()]));
}

#[test]
fn versions_with_odd_characters_are_refused() {
    let s = store("versions");
    for bad in ["1.0 && rm", "../../x", "v1;2", "1\n2", &"1".repeat(41)] {
        let m = newer(|m| item(m, "cli:kubectl").version = bad.into());
        assert!(refused(&s, &m).contains("version"), "{bad}");
    }
}

#[test]
fn duplicate_ids_and_missing_fields_are_refused() {
    let s = store("dups");
    let m = newer(|m| {
        let c = m.items[0].clone();
        m.items.push(c);
    });
    assert!(refused(&s, &m).contains("twice"));
    let mut zero = newer(|_| {});
    zero.sequence = 0;
    assert!(refused(&s, &zero).contains("sequence"));
    let mut bad_date = newer(|_| {});
    bad_date.issued = "soon".into();
    assert!(refused(&s, &bad_date).contains("date"));
    let mut schema = newer(|_| {});
    schema.schema = 2;
    assert!(refused(&s, &schema).contains("schema"));
}

// ---- rollback and the last good copy

#[test]
fn older_equal_and_back_dated_manifests_are_refused_and_the_last_good_one_is_kept() {
    let s = store("rollback");
    let v3 = newer(|m| {
        m.sequence = 3;
        item(m, "cli:kubectl").version = "v1.99.0".into();
    });
    let (b3, g3) = signed(&v3);
    s.apply(&b3, &g3).unwrap();
    // an older signed manifest (a replay)
    let v2 = newer(|m| item(m, "cli:kubectl").version = "v1.50.0".into());
    assert!(refused(&s, &v2).contains("older"));
    // the bundled sequence itself
    let v1 = bundled();
    assert!(refused(&s, &v1).contains("older") || refused(&s, &v1).contains("already used"));
    // the same sequence with other content
    let other = newer(|m| {
        m.sequence = 3;
        item(m, "cli:kubectl").version = "v1.98.0".into();
    });
    assert!(refused(&s, &other).contains("already used"));
    // a higher sequence with an earlier date
    let back = newer(|m| {
        m.sequence = 4;
        m.issued = "2026-10-29".into();
    });
    assert!(refused(&s, &back).contains("issue date"));
    // all of that left the good one in place, even after a restart
    assert_eq!(Store::new(&s.dir).active().items.iter().find(|i| i.id == "cli:kubectl").unwrap().version, "v1.99.0");
    // a newer one still goes in
    let v4 = newer(|m| {
        m.sequence = 4;
        m.issued = "2026-11-02".into();
    });
    assert!(matches!(s.apply(&signed(&v4).0, &signed(&v4).1), Ok(Applied::Updated { sequence: 4, .. })));
}

#[test]
fn a_damaged_or_swapped_saved_file_is_ignored_and_the_bundled_copy_is_used() {
    let s = store("damaged");
    let m = newer(|m| item(m, "cli:kubectl").version = "v1.99.0".into());
    let (b, g) = signed(&m);
    s.apply(&b, &g).unwrap();
    // someone edits the saved file
    let mut raw = std::fs::read(s.file()).unwrap();
    let at = String::from_utf8_lossy(&raw).find("v1.99.0").unwrap();
    raw[at + 3] = b'7';
    std::fs::write(s.file(), &raw).unwrap();
    assert_eq!(s.active(), bundled());
    // a file signed by another key
    let mut forged = sign_bytes(&other_key(), &b).into_bytes();
    forged.push(b'\n');
    forged.extend_from_slice(&b);
    std::fs::write(s.file(), forged).unwrap();
    assert_eq!(s.active(), bundled());
    // truncated
    std::fs::write(s.file(), b"abc").unwrap();
    assert_eq!(s.active(), bundled());
    // nothing left behind by the save
    assert!(std::fs::read_dir(&s.dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains(".tmp")));
}

#[test]
fn a_saved_manifest_older_than_a_newer_bundled_copy_is_not_used() {
    let d = dir("newer-iso");
    let old = Store::new(&d);
    let m = newer(|m| item(m, "cli:kubectl").version = "v1.99.0".into());
    old.apply(&signed(&m).0, &signed(&m).1).unwrap();
    // a later ISO bundles sequence 5 with its own data
    let mut b5 = bundled();
    b5.sequence = 5;
    b5.issued = "2026-12-01".into();
    let s = Store::with(&d, builtin_key(), b5.clone());
    assert_eq!(s.active(), b5);
}

// ---- the signing tool

#[test]
fn the_sign_and_verify_commands_work_from_a_key_path_given_at_run_time() {
    let d = dir("cli");
    std::fs::create_dir_all(&d).unwrap();
    let key = d.join("test.key");
    std::fs::write(&key, format!("{}\n", crate::pkg::signing_key_hex(&test_signing_key()))).unwrap();
    let man = d.join("downloads.json");
    std::fs::write(&man, to_json(&newer(|m| item(m, "cli:kubectl").version = "v1.99.0".into()))).unwrap();
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let out = run(&args(&["sign", man.to_str().unwrap(), "--key", key.to_str().unwrap()])).unwrap();
    assert!(out.contains("sequence 2"));
    let ok = run(&args(&["verify", man.to_str().unwrap()])).unwrap();
    assert!(ok.starts_with("OK") && ok.contains("1 items change"), "{ok}");
    let pk = crate::pkg::public_key_hex(&other_key());
    assert!(run(&args(&["verify", man.to_str().unwrap(), "--pubkey", &pk])).unwrap_err().contains("does not match"));
    // the tool refuses to sign what the rules refuse
    let mut bad = newer(|_| {});
    bad.items[0].licence = "MIT".into();
    std::fs::write(&man, to_json(&bad)).unwrap();
    assert!(run(&args(&["sign", man.to_str().unwrap(), "--key", key.to_str().unwrap()])).is_err());
    // keygen: a new file, mode 600, and it never overwrites
    let k2 = d.join("new.key");
    let out = run(&args(&["keygen", k2.to_str().unwrap()])).unwrap();
    assert!(out.contains("public key"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&k2).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(run(&args(&["keygen", k2.to_str().unwrap()])).is_err());
    assert!(run(&args(&["check"])).unwrap().contains("bundled manifest OK"));
}

#[test]
fn the_pin_command_records_our_own_sha256_for_an_exact_version_and_refuses_the_rest() {
    let d = dir("pin");
    std::fs::create_dir_all(&d).unwrap();
    let man = d.join("downloads.json");
    std::fs::write(&man, to_json(&bundled())).unwrap();
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let m = man.to_str().unwrap();
    let ver = bundled().items.into_iter().find(|i| i.id == "cli:duckdb").unwrap().version;
    // a local file: its SHA-256 is computed here (sha256 of "abc")
    let f = d.join("duckdb-wheel.whl");
    std::fs::write(&f, b"abc").unwrap();
    let pin = |extra: &[&str]| {
        let mut v = vec!["pin", m, "duckdb", "--version", ver.as_str(), "--source", "test source", "--date", "2026-10-28"];
        v.extend_from_slice(extra);
        run(&args(&v))
    };
    pin(&["--file", f.to_str().unwrap()]).unwrap();
    let it = parse(&std::fs::read(&man).unwrap()).unwrap().items.into_iter().find(|i| i.id == "cli:duckdb").unwrap();
    match it.checksum {
        Some(Checksum::Sha256 { files, date, source }) => {
            assert_eq!(files, vec![FileSum { file: "duckdb-wheel.whl".into(), sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into() }]);
            assert_eq!((date.as_str(), source.as_str()), ("2026-10-28", "test source"));
        }
        other => panic!("{other:?}"),
    }
    // a PyPI JSON adds every file; the same name is replaced, not doubled
    let j = d.join("p.json");
    std::fs::write(&j, r#"{"urls":[{"filename":"duckdb-wheel.whl","digests":{"sha256":"1111111111111111111111111111111111111111111111111111111111111111"}},{"filename":"other-1.0.tar.gz","digests":{"sha256":"2222222222222222222222222222222222222222222222222222222222222222"}}]}"#).unwrap();
    pin(&["--pypi-json", j.to_str().unwrap()]).unwrap();
    let it = parse(&std::fs::read(&man).unwrap()).unwrap().items.into_iter().find(|i| i.id == "cli:duckdb").unwrap();
    let Some(Checksum::Sha256 { files, .. }) = it.checksum else { panic!() };
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f.file == "duckdb-wheel.whl" && f.sha256.starts_with("1111")));
    // refused: a wrong version, a value that is not 64 hex characters, a float+sum item, an unknown item
    assert!(run(&args(&["pin", m, "duckdb", "--version", "0.0.1", "--sha256", &"a".repeat(64), "--as", "x.whl", "--source", "s", "--date", "2026-10-28"])).unwrap_err().contains("pinned to version"));
    assert!(pin(&["--sha256", "abc", "--as", "x.whl"]).is_err());
    assert!(run(&args(&["pin", m, "cli:kubectl", "--version", "v1", "--sha256", &"a".repeat(64), "--as", "x", "--source", "s", "--date", "2026-10-28"])).is_err());
    assert!(run(&args(&["pin", m, "cli:nothing", "--version", "v1", "--sha256", &"a".repeat(64), "--as", "x", "--source", "s", "--date", "2026-10-28"])).is_err());
}

#[test]
fn no_private_key_or_personal_path_is_in_the_manifest_files_or_this_module() {
    for f in ["legal/downloads-manifest.json", "legal/downloads-manifest.pub", "crates/iyakki-agent/src/dlmanifest.rs"] {
        let t = std::fs::read_to_string(repo().join(f)).unwrap();
        assert!(!t.contains("/home/") && !t.contains("BEGIN"), "{f}");
    }
}

// ---- the proof path: the version lookup of kubectl

struct Net(std::sync::atomic::AtomicUsize);

impl Streamer for Net {
    fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(url, "https://dl.k8s.io/release/stable.txt");
        Ok(StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(b"v1.31.0\n".to_vec())) })
    }
}

#[test]
fn kubectl_takes_its_version_from_the_manifest_and_falls_back_to_the_vendor_lookup() {
    let root = dir("kubectl");
    let m = crate::clis::CliManager::new(&root);
    let d = crate::clis::def("kubectl").unwrap();
    let net = Net(Default::default());
    // no fetched manifest: exactly the old lookup
    assert_eq!(m.latest_version(d, &net).unwrap(), "v1.31.0");
    assert_eq!(net.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    // a good fetched manifest names a version: no lookup at all
    let s = Store::new(&root.join("dlmanifest"));
    let n = newer(|m| item(m, "cli:kubectl").version = "v1.77.1".into());
    s.apply(&signed(&n).0, &signed(&n).1).unwrap();
    assert_eq!(m.latest_version(d, &net).unwrap(), "v1.77.1");
    assert_eq!(net.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    // a damaged manifest: back to the old lookup
    std::fs::write(root.join("dlmanifest/last-good.dlm"), b"broken").unwrap();
    assert_eq!(m.latest_version(d, &net).unwrap(), "v1.31.0");
    assert_eq!(net.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    // an item that is not wired yet ignores the manifest
    let n2 = newer(|m| item(m, "cli:terraform").version = "v9.9.9".into());
    let s2 = Store::new(&root.join("dlmanifest"));
    s2.apply(&signed(&n2).0, &signed(&n2).1).unwrap();
    assert_eq!(version_override_with(&s2, "terraform"), None);
}

// ---- fetching (the scripted network of netprobe; the check itself runs on the test thread)

mod fetching {
    use super::*;
    use crate::dlmanifest::fetch::*;
    use std::cell::RefCell;

    const URL: &str = "https://raw.githubusercontent.com/meisync/iyakki-downloads/main/downloads.json";

    fn st(name: &str) -> (Store, PathBuf) {
        let d = dir(name);
        (Store::with(&d, Some(test_signing_key().verifying_key()), bundled()), d)
    }

    fn check(s: &Store, d: &Path, now: u64) -> (Outcome, Vec<String>) {
        let lines = RefCell::new(Vec::new());
        let o = run_check(s, d, &[URL, ""], now, &|a, x| lines.borrow_mut().push(format!("{a} {x}")));
        (o, lines.into_inner())
    }

    fn serve(m: &Manifest, sig: Option<String>) {
        crate::netprobe::fake_reset();
        let (b, g) = signed(m);
        crate::netprobe::fake(&format!("{URL}.sig"), 200, &[], &sig.unwrap_or(g));
        crate::netprobe::fake(URL, 200, &[], std::str::from_utf8(&b).unwrap());
    }

    #[test]
    fn the_published_address_is_the_owners_and_the_second_place_is_empty() {
        assert_eq!(ADDRESSES[0], "https://raw.githubusercontent.com/meisync/iyakki-downloads/main/downloads.json");
        assert!(ADDRESSES[1].is_empty());
        assert_eq!(FETCH_MAX_BYTES, 256 * 1024);
    }

    #[test]
    fn a_good_newer_manifest_is_applied_saved_and_reported_in_words() {
        let (s, d) = st("fetch-ok");
        serve(&newer(|m| item(m, "cli:helm").version = "v3.99.0".into()), None);
        let (o, audit) = check(&s, &d, 1000);
        assert_eq!(o, Outcome::Updated { sequence: 2, changed: vec!["cli:helm".into()] });
        assert!(audit[0].starts_with("dlmanifest.fetch.ok "));
        assert_eq!(version_override_with(&s, "helm").as_deref(), Some("v3.99.0"));
        let t = status(&d);
        assert_eq!((t.at, t.result.as_str()), (Some(1000), "updated"));
        assert_eq!(t.notice.as_deref(), Some("Download list updated: 1 item changed (helm)"));
        assert_eq!(status_line(&t, 1000 + 3 * 3600), "Checked 3 h ago: updated");
        // the same file again: unchanged, and the notice stays
        let (o, audit) = check(&s, &d, 2000);
        assert_eq!(o, Outcome::Unchanged);
        assert!(audit[0].starts_with("dlmanifest.fetch.unchanged"));
        assert!(status(&d).notice.is_some());
    }

    #[test]
    fn a_wrong_signature_a_rollback_and_a_too_large_file_change_nothing() {
        let (s, d) = st("fetch-refused");
        let bad = sign_bytes(&other_key(), b"x");
        serve(&newer(|m| item(m, "cli:helm").version = "v3.99.0".into()), Some(bad));
        let (o, audit) = check(&s, &d, 1);
        assert!(matches!(o, Outcome::Refused(_)));
        assert_eq!(audit[0], "dlmanifest.fetch.refused signature did not match");
        assert!(!d.join("last-good.dlm").exists());
        assert_eq!(status(&d).result, "refused: signature did not match");
        assert_eq!(version_override_with(&s, "helm"), None);
        // rollback: sequence 1 with different content
        serve(&{ let mut m = bundled(); item(&mut m, "cli:helm").version = "v1.0.0".into(); m }, None);
        let (o, _) = check(&s, &d, 2);
        assert!(matches!(o, Outcome::Refused(_)), "{o:?}");
        assert!(!d.join("last-good.dlm").exists());
        // too large
        crate::netprobe::fake_reset();
        crate::netprobe::fake(URL, 200, &[], &" ".repeat(FETCH_MAX_BYTES + 1));
        let (o, audit) = check(&s, &d, 3);
        assert_eq!(o, Outcome::Refused("the manifest is too large".into()));
        assert!(audit[0].contains("dlmanifest.fetch.refused"));
        assert!(!d.join("last-good.dlm").exists());
    }

    #[test]
    fn no_network_and_a_404_are_a_quiet_no_update() {
        let (s, d) = st("fetch-offline");
        crate::netprobe::fake_reset();
        let (o, audit) = check(&s, &d, 5);
        assert!(matches!(o, Outcome::Offline(_)));
        assert!(audit[0].starts_with("dlmanifest.fetch.offline"));
        assert_eq!(status(&d).result, "no connection");
        crate::netprobe::fake_reset();
        crate::netprobe::fake(URL, 404, &[], "404: Not Found");
        let (o, audit) = check(&s, &d, 6);
        assert_eq!(o, Outcome::Offline("HTTP 404".into()));
        assert!(audit[0].ends_with("HTTP 404"));
        assert!(!d.join("last-good.dlm").exists());
        // signature file missing
        crate::netprobe::fake_reset();
        let (b, _) = signed(&newer(|_| {}));
        crate::netprobe::fake(&format!("{URL}.sig"), 404, &[], "");
        crate::netprobe::fake(URL, 200, &[], std::str::from_utf8(&b).unwrap());
        assert!(matches!(check(&s, &d, 7).0, Outcome::Offline(_)));
    }

    #[test]
    fn a_check_on_its_own_comes_once_per_day_and_sooner_after_no_connection() {
        let (s, d) = st("fetch-due");
        assert!(due(&d, 100));
        crate::netprobe::fake_reset();
        check(&s, &d, 1000);
        assert!(!due(&d, 1000 + 1800));
        assert!(due(&d, 1000 + RETRY_OFFLINE_S));
        serve(&newer(|_| {}), None);
        check(&s, &d, 5000);
        assert!(!due(&d, 5000 + EVERY_S - 1));
        assert!(due(&d, 5000 + EVERY_S));
        // not due and not forced: nothing starts
        let real = now_s();
        write_state_for_test(&d, real);
        assert!(!start(&d, false));
    }

    fn write_state_for_test(d: &Path, at: u64) {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("fetch-state.json"), format!("{{\"at\":{at},\"result\":\"up to date\",\"notice\":null}}")).unwrap();
    }

    #[test]
    fn times_and_notices_read_in_plain_words() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(700, 100), "10 min ago");
        assert_eq!(ago(100 + 2 * 86400, 100), "2 days ago");
        assert_eq!(status_line(&CheckStatus::default(), 5), "Not checked yet");
        assert_eq!(status_line(&CheckStatus { running: true, ..Default::default() }, 5), "Checking for a newer download list...");
        let many: Vec<String> = (0..8).map(|i| format!("cli:t{i}")).collect();
        assert_eq!(notice_for(&many), "Download list updated: 8 items changed (t0, t1, t2, t3, t4, t5, and 2 more)");
    }

    #[test]
    fn helm_takes_its_version_from_the_manifest_and_falls_back_without_one() {
        let (s, _d) = st("fetch-helm");
        assert_eq!(version_override_with(&s, "helm"), None);
        let (b, g) = signed(&newer(|m| item(m, "cli:helm").version = "v3.99.0".into()));
        s.apply(&b, &g).unwrap();
        assert_eq!(version_override_with(&s, "helm").as_deref(), Some("v3.99.0"));
        assert_eq!(version_override_with(&s, "terraform"), None);
    }
}
