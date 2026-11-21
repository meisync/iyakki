use super::tests::rig;
use super::*;
use iyakki_volcrypt::luks2::parse_header;
use iyakki_volcrypt::luks2_export::{DATA_OFFSET, HEADER_COPY};
use std::os::unix::fs::FileExt;

fn open_export(path: &Path, pw: &str) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).unwrap();
    let mut head = vec![0u8; HEADER_COPY];
    f.read_exact_at(&mut head, 0).unwrap();
    let h = parse_header(&head).map_err(|e| e.to_string())?;
    let mut read = |o: u64, n: usize| -> Option<Vec<u8>> {
        let mut b = vec![0u8; n];
        f.read_exact_at(&mut b, o).ok()?;
        Some(b)
    };
    h.unlock(pw.as_bytes(), &mut read).map(|k| k.to_vec()).map_err(|e| e.to_string())
}

#[test]
fn an_export_is_a_luks2_file_with_the_same_payload_that_opens_with_the_password_and_the_key_is_never_in_the_report() {
    let r = rig("export");
    r.made("ann", "pw one");
    // an open home is refused (its blocks are changing); a locked one is exported
    assert_eq!(r.env.export_luks2("ann", "pw one", &r.env.export_path("ann"), &mut |_, _| {}).unwrap_err(), HomeError::InUse);
    r.env.lock("ann").unwrap();
    // a file written through the open home ends up encrypted in the payload
    let dest = r.env.export_path("ann");
    let mut seen = vec![];
    let rep = r.env.export_luks2("ann", "pw one", &dest, &mut |d, t| seen.push((d, t))).unwrap();
    assert_eq!(rep.bytes, DATA_OFFSET + rep.payload_bytes);
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), rep.bytes);
    assert_eq!(seen.last().map(|(d, t)| d == t), Some(true));
    assert_eq!(rep.uuid.len(), 36);
    // the header opens with the password, not with another one, and the payload equals the container's
    let key = open_export(&dest, "pw one").unwrap();
    assert_eq!(key.len(), 64);
    assert!(open_export(&dest, "pw two").is_err());
    let c = std::fs::File::open(r.env.container_path("ann")).unwrap();
    let (mut a, mut b) = (vec![0u8; 8192], vec![0u8; 8192]);
    let h = std::fs::File::open(&dest).unwrap();
    for at in [0u64, 4096, rep.payload_bytes / 2, rep.payload_bytes - 8192] {
        c.read_exact_at(&mut a, PAYLOAD_OFFSET + at).unwrap();
        h.read_exact_at(&mut b, DATA_OFFSET + at).unwrap();
        assert_eq!(a, b);
    }
    // the report and its debug text hold no key bytes
    let text = format!("{rep:?}");
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!text.contains(&hex));
    // the file is private to the owner
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn an_export_refuses_a_wrong_password_an_existing_file_and_a_missing_home_and_leaves_no_partial_file() {
    let r = rig("export-refuse");
    r.made("bob", "right");
    r.env.lock("bob").unwrap();
    let dest = r.env.export_path("bob");
    assert_eq!(r.env.export_luks2("bob", "wrong", &dest, &mut |_, _| {}).unwrap_err(), HomeError::WrongSecret);
    assert_eq!(r.env.export_luks2("bob", "", &dest, &mut |_, _| {}).unwrap_err(), HomeError::WrongSecret);
    assert!(!dest.exists());
    // the wrong attempt counted against the same throttle as signing in
    assert!(matches!(r.env.export_luks2("bob", "right", &dest, &mut |_, _| {}).unwrap_err(), HomeError::Throttled(_)));
    r.clock.fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(r.env.export_luks2("nobody", "x", &r.env.export_path("nobody"), &mut |_, _| {}).unwrap_err(), HomeError::NoHome);
    r.env.export_luks2("bob", "right", &dest, &mut |_, _| {}).unwrap();
    let before = std::fs::read(&dest).unwrap();
    let e = r.env.export_luks2("bob", "right", &dest, &mut |_, _| {}).unwrap_err();
    assert!(matches!(&e, HomeError::Io(t) if t.contains("never overwrites")), "{e:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), before);
}
