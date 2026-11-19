//! HOTP, TOTP, base32 and otpauth links against the published test vectors and against bad input.

use super::*;

const K1: &[u8] = b"12345678901234567890";
const K256: &[u8] = b"12345678901234567890123456789012";
const K512: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

#[test]
fn hotp_matches_the_rfc_4226_vectors() {
    let want = ["755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489"];
    for (i, w) in want.iter().enumerate() {
        assert_eq!(hotp(Algo::Sha1, K1, i as u64, 6), *w, "counter {i}");
    }
}

#[test]
fn totp_matches_the_rfc_6238_vectors_for_all_three_hashes() {
    // time, SHA-1, SHA-256, SHA-512 (8 digits, 30 second step)
    let rows: [(i64, &str, &str, &str); 6] = [
        (59, "94287082", "46119246", "90693936"),
        (1_111_111_109, "07081804", "68084774", "25091201"),
        (1_111_111_111, "14050471", "67062674", "99943326"),
        (1_234_567_890, "89005924", "91819424", "93441116"),
        (2_000_000_000, "69279037", "90698825", "38618901"),
        (20_000_000_000, "65353130", "77737706", "47863826"),
    ];
    for (t, a, b, c) in rows {
        assert_eq!(totp(Algo::Sha1, K1, t, 30, 8), a, "SHA1 at {t}");
        assert_eq!(totp(Algo::Sha256, K256, t, 30, 8), b, "SHA256 at {t}");
        assert_eq!(totp(Algo::Sha512, K512, t, 30, 8), c, "SHA512 at {t}");
    }
}

#[test]
fn hmac_matches_rfc_4231_case_2_and_long_keys_are_hashed_first() {
    let hex = |b: Vec<u8>| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(hmac(Algo::Sha256, b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    assert_eq!(
        hex(hmac(Algo::Sha512, b"Jefe", b"what do ya want for nothing?")),
        "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
    );
    assert_eq!(hex(hmac(Algo::Sha1, b"Jefe", b"what do ya want for nothing?")), "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
    // a key longer than the block is the hash of the key
    let long = vec![0xaa; 131];
    let short = Sha256::digest(&long[..]);
    assert_eq!(hmac(Algo::Sha256, &long, b"x"), hmac(Algo::Sha256, &short, b"x"));
}

#[test]
fn the_steps_and_the_seconds_left() {
    assert_eq!(seconds_left(0, 30), 30);
    assert_eq!(seconds_left(29, 30), 1);
    assert_eq!(seconds_left(30, 30), 30);
    assert_eq!(seconds_left(59, 30), 1);
    assert_eq!(seconds_left(-5, 30), 30, "a time before 1970 does not panic");
    assert_eq!(seconds_left(10, 0), 1, "a zero step is treated as one second");
    // the code is the same inside one step and changes at its edge
    assert_eq!(totp(Algo::Sha1, K1, 60, 30, 6), totp(Algo::Sha1, K1, 89, 30, 6));
    assert_ne!(totp(Algo::Sha1, K1, 89, 30, 6), totp(Algo::Sha1, K1, 90, 30, 6));
    assert_eq!(totp(Algo::Sha1, K1, 59, 30, 6).len(), 6);
    assert_eq!(totp(Algo::Sha1, K1, i64::MIN, 30, 6), totp(Algo::Sha1, K1, 0, 30, 6));
}

#[test]
fn codes_are_grouped_for_reading() {
    assert_eq!(grouped("123456"), "123 456");
    assert_eq!(grouped("12345678"), "1234 5678");
    assert_eq!(grouped("1234567"), "123 4567");
    assert_eq!(grouped("12"), "12");
}

#[test]
fn base32_matches_the_rfc_4648_vectors() {
    for (plain, enc) in [("", ""), ("f", "MY======"), ("fo", "MZXQ===="), ("foo", "MZXW6==="), ("foob", "MZXW6YQ="), ("fooba", "MZXW6YTB"), ("foobar", "MZXW6YTBOI======")] {
        assert_eq!(base32_encode(plain.as_bytes()), enc);
        if !plain.is_empty() {
            assert_eq!(base32_decode(enc).unwrap().as_slice(), plain.as_bytes(), "{enc}");
        }
    }
}

#[test]
fn base32_reading_is_lenient_about_spaces_case_and_padding() {
    let want = b"foobar".to_vec();
    for t in ["MZXW6YTBOI", "mzxw6ytboi", "MZXW 6YTB OI", "mzxw-6ytb-oi==", "  MZXW6YTBOI======\n", "MzXw6YtBoI"] {
        assert_eq!(base32_decode(t).unwrap().as_slice(), want.as_slice(), "{t:?}");
    }
    assert_eq!(normalize_secret("jbsw y3dp ehpk 3pxp").unwrap().as_str(), "JBSWY3DPEHPK3PXP");
    assert_eq!(normalize_secret("MZXW6YQ=").unwrap().as_str(), "MZXW6YQ");
}

#[test]
fn base32_refuses_what_is_not_base32() {
    for bad in ["", "   ", "====", "MZXW6YT1", "MZXW6YT8", "hello!", "MZXW6\u{e9}", "M", "MZX", "MZXW6Y"] {
        assert!(base32_decode(bad).is_err(), "{bad:?}");
    }
    let huge = "A".repeat(1_000_000);
    assert!(base32_decode(&huge).unwrap_err().contains("too long"));
    // no message repeats what was typed
    assert!(!base32_decode("MZXW6YT1").unwrap_err().contains("MZXW6YT1"));
}

#[test]
fn algorithm_names_are_read_in_the_usual_spellings() {
    for (t, a) in [("SHA1", Algo::Sha1), ("sha-1", Algo::Sha1), ("HmacSHA256", Algo::Sha256), ("sha256", Algo::Sha256), ("SHA-512", Algo::Sha512)] {
        assert_eq!(Algo::parse(t), Some(a), "{t}");
    }
    assert_eq!(Algo::parse("MD5"), None);
    assert_eq!(Algo::parse(""), None);
    for a in Algo::ALL {
        assert_eq!(Algo::parse(a.name()), Some(a));
    }
}

#[test]
fn a_full_otpauth_link_is_read() {
    let p = parse_otpauth("otpauth://totp/Example%20Co:alice%40example.com?secret=jbsw y3dp&issuer=Example%20Co&algorithm=SHA256&digits=8&period=60").unwrap();
    assert_eq!((p.issuer.as_str(), p.account.as_str()), ("Example Co", "alice@example.com"));
    assert_eq!(p.secret.as_str(), "JBSWY3DP");
    assert_eq!((p.algo, p.digits, p.period), (Algo::Sha256, 8, 60));
}

#[test]
fn the_defaults_and_the_label_forms() {
    let p = parse_otpauth("  OTPAUTH://TOTP/GitHub:bob?secret=JBSWY3DPEHPK3PXP  ").unwrap();
    assert_eq!((p.issuer.as_str(), p.account.as_str(), p.algo, p.digits, p.period), ("GitHub", "bob", Algo::Sha1, 6, 30));
    // %3A for the colon, spaces after it, the issuer parameter wins over the label's prefix
    let p = parse_otpauth("otpauth://totp/Old%3A%20carol?secret=JBSWY3DP&issuer=New+Name").unwrap();
    assert_eq!((p.issuer.as_str(), p.account.as_str()), ("New Name", "carol"));
    // no issuer anywhere: just the account
    let p = parse_otpauth("otpauth://totp/dave?secret=JBSWY3DP").unwrap();
    assert_eq!((p.issuer.as_str(), p.account.as_str()), ("", "dave"));
    // unknown parameters and a fragment are ignored; non-ASCII names are fine
    let p = parse_otpauth("otpauth://totp/B%C3%BCro:%C3%A9l?secret=JBSWY3DP&image=x&foo#frag").unwrap();
    assert_eq!((p.issuer.as_str(), p.account.as_str()), ("Büro", "él"));
}

#[test]
fn bad_links_are_refused_with_plain_sentences_and_never_repeat_the_secret() {
    let bad = [
        "",
        "hello",
        "https://example.com/?secret=JBSWY3DP",
        "otpauth://totp",
        "otpauth://totp/",
        "otpauth://totp/x",
        "otpauth://totp/x?secret=",
        "otpauth://totp/x?secret=NOT*BASE32",
        "otpauth://totp/x?secret=JBSWY3DP&digits=5",
        "otpauth://totp/x?secret=JBSWY3DP&digits=9",
        "otpauth://totp/x?secret=JBSWY3DP&digits=abc",
        "otpauth://totp/x?secret=JBSWY3DP&period=0",
        "otpauth://totp/x?secret=JBSWY3DP&period=99999",
        "otpauth://totp/x?secret=JBSWY3DP&algorithm=MD5",
        "otpauth://totp/x%ZZ?secret=JBSWY3DP",
        "otpauth://totp/x%?secret=JBSWY3DP",
        "otpauth://totp/%FF?secret=JBSWY3DP",
        "otpauth://totp/?secret=JBSWY3DP",
        "otpauth://hotp/x?secret=JBSWY3DP&counter=1",
        "otpauth://other/x?secret=JBSWY3DP",
        "otpauth://totp/x%0Ay?secret=JBSWY3DP",
    ];
    for b in bad {
        let e = parse_otpauth(b).expect_err(b);
        assert!(!e.contains("JBSWY3DP") && !e.contains("NOT*BASE32"), "{b}: {e}");
    }
    assert!(parse_otpauth("otpauth://hotp/x?secret=JBSWY3DP").unwrap_err().contains("HOTP"));
}

#[test]
fn huge_input_is_refused_quickly() {
    let t = std::time::Instant::now();
    let big = format!("otpauth://totp/x?secret={}", "A".repeat(5_000_000));
    assert!(parse_otpauth(&big).unwrap_err().contains("too long"));
    let long_name = format!("otpauth://totp/{}?secret=JBSWY3DP", "n".repeat(200));
    assert!(parse_otpauth(&long_name).is_err());
    let many = format!("otpauth://totp/x?{}secret=JBSWY3DP", "a=b&".repeat(400));
    assert!(parse_otpauth(&many).is_ok());
    assert!(t.elapsed().as_secs() < 2);
}

#[test]
fn a_link_gives_the_code_the_rfc_says() {
    // the SHA-1 key of the RFC in base32 through a link
    let p = parse_otpauth(&format!("otpauth://totp/rfc?secret={}&digits=8", base32_encode(K1))).unwrap();
    let key = base32_decode(&p.secret).unwrap();
    assert_eq!(totp(p.algo, &key, 59, p.period, p.digits), "94287082");
}

#[test]
fn the_debug_text_hides_the_secret() {
    let p = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").unwrap();
    let d = format!("{p:?}");
    assert!(!d.contains("JBSWY3DPEHPK3PXP") && d.contains("hidden"), "{d}");
}

#[test]
fn the_link_check_is_case_blind_and_safe_on_short_or_odd_text() {
    assert!(looks_like_link("otpauth://totp/x"));
    assert!(looks_like_link(" OtpAuth://x"));
    assert!(!looks_like_link("otpauth:/"));
    assert!(!looks_like_link("héllo wörld ünïcode"));
    assert!(!looks_like_link(""));
}
