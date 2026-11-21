//! The QR code tool's texts and symbols (the picture and page tests stay with the tile in iyakki-apps).

use crate::qrcode::{make, payload, PRESETS};
use iyakki_qr::qr::{Level, WifiSecurity};

fn pay(preset: usize, typed: &str) -> Result<String, String> {
    payload(preset, typed, WifiSecurity::Wpa, false)
}

#[test]
fn every_kind_of_code_builds_its_text() {
    assert_eq!(pay(0, "hello; world").unwrap(), "hello; world", "plain text is kept as it is");
    assert_eq!(pay(1, " iyakki.test/a?b=1 ").unwrap(), "https://iyakki.test/a?b=1");
    assert_eq!(pay(1, "http://iyakki.test").unwrap(), "http://iyakki.test", "a scheme already there stays");
    assert!(pay(1, "two words").unwrap_err().contains("no spaces"));
    assert_eq!(pay(3, "+49 (30) 123-4567").unwrap(), "tel:+49301234567");
    assert!(pay(3, "12").unwrap_err().contains("three digits"));
    assert!(pay(3, "12a45").unwrap_err().contains("does not belong"));
    assert_eq!(pay(4, "ada@example.org").unwrap(), "mailto:ada@example.org");
    assert_eq!(pay(4, "ada@example.org\nHi there\nLine 1\nLine 2").unwrap(), "mailto:ada@example.org?subject=Hi%20there&body=Line%201%0ALine%202");
    assert!(pay(4, "ada.example.org").unwrap_err().contains("e-mail address"));
    assert_eq!(pay(5, "+49 30 1234567\nSee you at 5, ok?").unwrap(), "sms:+49301234567?body=See%20you%20at%205%2C%20ok%3F");
    assert_eq!(pay(5, "0301234567").unwrap(), "sms:0301234567");
    assert_eq!(pay(6, "52.5200, 13.4050").unwrap(), "geo:52.52,13.405");
    assert_eq!(pay(6, "-33.86 151.21").unwrap(), "geo:-33.86,151.21");
    assert!(pay(6, "91, 10").unwrap_err().contains("-90 and 90"));
    assert!(pay(6, "10, 181").unwrap_err().contains("-180 and 180"));
    assert!(pay(6, "10").unwrap_err().contains("two numbers"));
    assert!(pay(6, "ten, 20").unwrap_err().contains("not a number"));
    assert_eq!(
        pay(7, "Ada Lovelace\n+44 20 7946 0000\nada@example.org").unwrap(),
        "BEGIN:VCARD\nVERSION:3.0\nN:Lovelace;Ada;;;\nFN:Ada Lovelace\nTEL:+442079460000\nEMAIL:ada@example.org\nEND:VCARD"
    );
    assert_eq!(pay(7, "Madonna").unwrap(), "BEGIN:VCARD\nVERSION:3.0\nN:;Madonna;;;\nFN:Madonna\nEND:VCARD");
    assert!(pay(7, "Smith; John, Jr").unwrap().contains("FN:Smith\\; John\\, Jr"), "the card's own separators are escaped");
    // nothing typed is nothing to make, except the Text kind which is just empty too
    for p in 0..PRESETS.len() {
        assert_eq!(pay(p, "  \n ").unwrap(), if p == 0 { "  \n " } else { "" }, "{}", PRESETS[p].name);
    }
}

#[test]
fn a_wifi_code_escapes_what_the_format_reserves_and_follows_the_security_choice() {
    let w = |t: &str, s, h| payload(2, t, s, h);
    assert_eq!(w("home\npass word", WifiSecurity::Wpa, false).unwrap(), "WIFI:T:WPA;S:home;P:pass word;;");
    assert_eq!(w("a;b,c:d\\e\"f\nx;y", WifiSecurity::Wpa, false).unwrap(), "WIFI:T:WPA;S:a\\;b\\,c\\:d\\\\e\\\"f;P:x\\;y;;");
    assert_eq!(w("home\nsecret", WifiSecurity::Wep, true).unwrap(), "WIFI:T:WEP;S:home;P:secret;H:true;;");
    assert_eq!(w("cafe", WifiSecurity::Open, false).unwrap(), "WIFI:T:nopass;S:cafe;;");
    assert_eq!(w("cafe\nignored", WifiSecurity::Open, true).unwrap(), "WIFI:T:nopass;S:cafe;H:true;;", "an open network has no password part");
    assert!(w("home", WifiSecurity::Wpa, false).unwrap_err().contains("password on the second line"));
    assert!(w("\nonly-a-password", WifiSecurity::Wpa, false).unwrap_err().contains("network name"));
    assert!(w(&"n".repeat(33), WifiSecurity::Open, false).unwrap_err().contains("32 bytes"));
    // a password with spaces at its ends keeps them, and a line break from a pasted Windows file is dropped
    assert_eq!(w("home\r\n  pw \r\n", WifiSecurity::Wpa, false).unwrap(), "WIFI:T:WPA;S:home;P:  pw ;;");
}

#[test]
fn the_status_line_names_version_size_bytes_and_a_text_that_is_too_long() {
    let m = make(0, "HELLO", Level::M, WifiSecurity::Wpa, false);
    assert_eq!(m.status, "Version 1, 21 x 21 modules, level M, about 15%, 5 of 14 bytes used.");
    assert!(m.qr.is_some() && !m.err);
    let m = make(0, &"x".repeat(2954), Level::L, WifiSecurity::Wpa, false);
    assert!(m.qr.is_none() && m.err, "{}", m.status);
    assert_eq!(m.status, "Too long for a QR code: 2954 bytes, and level L holds at most 2953. Shorten the text.");
    let m = make(0, &"x".repeat(1300), Level::H, WifiSecurity::Wpa, false);
    assert!(m.status.starts_with("Too long for a QR code: 1300 bytes, and level H holds at most 1273.") && m.status.contains("lower error correction"), "{}", m.status);
    assert!(make(0, &"x".repeat(1273), Level::H, WifiSecurity::Wpa, false).qr.is_some());
    let m = make(0, "", Level::M, WifiSecurity::Wpa, false);
    assert!(m.qr.is_none() && !m.err && m.status.contains("Type something"));
    let m = make(3, "12", Level::M, WifiSecurity::Wpa, false);
    assert!(m.qr.is_none() && m.err);
}

