use super::*;
use std::os::unix::fs::PermissionsExt;

const NOW: i64 = 1_800_000_000;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-clientlink-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed([n; 32])
}

#[test]
fn the_endpoint_is_off_until_switched_on() {
    let d = dir("switch");
    assert!(!enabled(&d));
    set_enabled(&d, true).unwrap();
    assert!(enabled(&d));
    set_enabled(&d, false).unwrap();
    assert!(!enabled(&d));
    set_enabled(&d, false).unwrap();
}

#[test]
fn a_token_works_once_and_is_stored_only_as_a_hash() {
    let d = dir("once");
    let t = create_token(&d, "ann", NOW, TOKEN_TTL_SECS).unwrap();
    assert_eq!(t.len(), 64);
    let file = std::fs::read_to_string(d.join("client/tokens.json")).unwrap();
    assert!(!file.contains(&t), "the token itself is never written down");
    assert_eq!(std::fs::metadata(d.join("client/tokens.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(open_tokens(&d, NOW), 1);
    assert_eq!(redeem_token(&d, &t, NOW + 5), Ok("ann".to_string()));
    assert_eq!(redeem_token(&d, &t, NOW + 6), Err(TokenError::Used));
    assert_eq!(open_tokens(&d, NOW), 0);
}

#[test]
fn an_unknown_or_expired_token_is_refused_and_expiry_is_in_minutes() {
    let d = dir("expiry");
    assert_eq!(redeem_token(&d, &"0".repeat(64), NOW), Err(TokenError::Unknown));
    let t = create_token(&d, "ann", NOW, TOKEN_TTL_SECS).unwrap();
    assert_eq!(redeem_token(&d, &t, NOW + TOKEN_TTL_SECS), Err(TokenError::Expired));
    assert_eq!(redeem_token(&d, &t, NOW + TOKEN_TTL_SECS - 1), Ok("ann".to_string()), "just inside the time");
    // a very long wish is cut to the maximum
    let t2 = create_token(&d, "ann", NOW, 100_000).unwrap();
    assert_eq!(redeem_token(&d, &t2, NOW + TOKEN_MAX_TTL_SECS), Err(TokenError::Expired));
}

#[test]
fn only_a_few_links_can_be_open_at_once() {
    let d = dir("many");
    for _ in 0..MAX_OPEN_TOKENS {
        create_token(&d, "ann", NOW, 60).unwrap();
    }
    assert!(create_token(&d, "ann", NOW, 60).is_err());
    assert!(create_token(&d, "ann", NOW + 61, 60).is_ok(), "expired ones make room");
}

#[test]
fn the_link_keeps_the_token_in_the_fragment() {
    assert_eq!(link_url("https://box.local:8443/", "abc"), "https://box.local:8443/connect#abc");
}

#[test]
fn a_device_is_enrolled_listed_touched_and_revoked() {
    let d = dir("devices");
    let k = key(1);
    let dev = enrol_device(&d, "  Maya's laptop ", &k.public_hex(), "ann", NOW).unwrap();
    assert_eq!((dev.name.as_str(), dev.by.as_str(), dev.last_seen, dev.active()), ("Maya's laptop", "ann", 0, true));
    assert_eq!(std::fs::metadata(d.join("client/devices.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(enrol_device(&d, "again", &k.public_hex(), "ann", NOW).unwrap_err().contains("already"));
    touch_device(&d, &dev.id, NOW + 100);
    touch_device(&d, &dev.id, NOW + 110); // too soon: not written
    assert_eq!(get_device(&d, &dev.id).unwrap().last_seen, NOW + 100);
    assert_eq!(revoke_device(&d, &dev.id, NOW + 200), Ok(true));
    assert_eq!(revoke_device(&d, &dev.id, NOW + 201), Ok(false));
    let l = list_devices(&d);
    assert_eq!((l.len(), l[0].revoked), (1, Some(NOW + 200)));
    // the same key may be enrolled again once the old record is revoked
    assert!(enrol_device(&d, "new", &k.public_hex(), "ann", NOW + 300).is_ok());
}

#[test]
fn bad_names_and_keys_and_too_many_devices_are_refused() {
    let d = dir("bad");
    let k = key(2).public_hex();
    assert!(enrol_device(&d, "", &k, "a", NOW).is_err());
    assert!(enrol_device(&d, &"x".repeat(61), &k, "a", NOW).is_err());
    assert!(enrol_device(&d, "bad\nname", &k, "a", NOW).is_err());
    assert!(enrol_device(&d, "ok", "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ x", "a", NOW).is_err());
    assert!(enrol_device(&d, "ok", "nonsense", "a", NOW).is_err());
    for i in 0..MAX_DEVICES as u8 {
        enrol_device(&d, "d", &key(10 + i).public_hex(), "a", NOW).unwrap();
    }
    assert!(enrol_device(&d, "d", &key(99).public_hex(), "a", NOW).unwrap_err().contains("at most"));
}

#[test]
fn a_signed_answer_is_for_this_device_this_challenge_and_this_time() {
    let d = dir("sig");
    let k = key(3);
    let dev = enrol_device(&d, "x", &k.public_hex(), "a", NOW).unwrap();
    let now = NOW as u64;
    let ch = new_challenge().unwrap();
    let good = sign_answer(&k, &dev.id, &ch, now).unwrap();
    assert_eq!(verify_answer(&dev, &good, &ch, now), Ok(()));
    // another challenge (a captured answer on another connection)
    assert_eq!(verify_answer(&dev, &good, &new_challenge().unwrap(), now), Err(AuthError::BadSignature));
    // another key
    let other = sign_answer(&key(4), &dev.id, &ch, now).unwrap();
    assert_eq!(verify_answer(&dev, &other, &ch, now), Err(AuthError::BadSignature));
    // another device id, junk, old and future times
    assert_eq!(verify_answer(&dev, &sign_answer(&k, "dother", &ch, now).unwrap(), &ch, now), Err(AuthError::Malformed));
    assert_eq!(verify_answer(&dev, "junk", &ch, now), Err(AuthError::Malformed));
    assert_eq!(verify_answer(&dev, &sign_answer(&k, &dev.id, &ch, now - SIG_WINDOW_SECS - 1).unwrap(), &ch, now), Err(AuthError::BadSignature));
    assert_eq!(verify_answer(&dev, &sign_answer(&k, &dev.id, &ch, now + SIG_WINDOW_SECS + 1).unwrap(), &ch, now), Err(AuthError::BadSignature));
    // a signature for another route (the web sign-in) does not count
    let web = k.sign_request(&dev.id, "POST", "/api/login-key", now, ch.as_bytes()).unwrap();
    assert_eq!(verify_answer(&dev, &web, &ch, now), Err(AuthError::BadSignature));
}

#[test]
fn the_link_can_carry_the_certificate_pin_in_the_query_and_the_token_stays_in_the_fragment() {
    let fp = "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89";
    let hex = fp.replace(':', "").to_ascii_lowercase();
    assert_eq!(link_url_pinned("https://box.local:8443/", "abc", Some(fp)), format!("https://box.local:8443/connect?pin={hex}#abc"));
    assert_eq!(link_url_pinned("https://box.local:8443", "abc", Some(&hex)), format!("https://box.local:8443/connect?pin={hex}#abc"));
    // a missing or malformed fingerprint leaves the plain link
    for bad in [None, Some(""), Some("AB:CD"), Some("zz"), Some(&format!("{hex}00")[..])] {
        assert_eq!(link_url_pinned("https://box.local:8443", "abc", bad), "https://box.local:8443/connect#abc");
    }
}

#[test]
fn a_lock_file_of_another_program_is_waited_for_and_one_left_by_a_crash_is_taken_over() {
    let d = dir("filelock");
    let tok = create_token(&d, "ann", NOW, 600).unwrap();
    // another program holds the lock for a moment: the token is spent after it lets go, not before
    let lockfile = folder(&d).join("lock");
    std::fs::write(&lockfile, "").unwrap();
    let lf = lockfile.clone();
    let let_go = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lg = let_go.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        lg.store(true, std::sync::atomic::Ordering::SeqCst);
        std::fs::remove_file(lf).unwrap();
    });
    assert_eq!(redeem_token(&d, &tok, NOW + 1).unwrap(), "ann");
    assert!(let_go.load(std::sync::atomic::Ordering::SeqCst), "it waited for the other holder to let go (no stopwatch: the holder's flag shows the order)");
    t.join().unwrap();
    assert!(!lockfile.exists(), "the lock is given up after use");
    // a lock file nobody can still be holding (older than ten seconds) does not block anything: it is taken over and removed after use
    // (a lock that was waited out instead would still be there)
    let tok2 = create_token(&d, "ann", NOW, 600).unwrap();
    let f = std::fs::File::create(&lockfile).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(redeem_token(&d, &tok2, NOW + 2).unwrap(), "ann");
    assert!(!lockfile.exists(), "the stale lock was taken over and given up after use");
    // the same token cannot be spent twice, whatever the lock does
    assert!(matches!(redeem_token(&d, &tok2, NOW + 3), Err(TokenError::Used)));
}
