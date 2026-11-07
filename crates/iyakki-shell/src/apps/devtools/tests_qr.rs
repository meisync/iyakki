//! The QR code tool: the texts the codes hold, the symbol, the two pictures, the page at several sizes and the saves.

use super::*;
use iyakki_agent::qr::{Level, WifiSecurity};
use qrcode::{make, payload, PRESETS};

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

#[test]
fn the_pictures_are_valid_and_scan_back_to_the_text() {
    let text = "WIFI:T:WPA;S:iyakki;P:correct horse;;";
    for level in Level::ALL {
        let q = iyakki_agent::qr::encode_level(text.as_bytes(), level).unwrap();
        // SVG: well-formed enough, white square, one black path with a run for every dark row
        let svg = qrcode::svg(&q);
        let n = q.size + 8;
        assert!(svg.starts_with("<?xml") && svg.contains(&format!("viewBox=\"0 0 {n} {n}\"")) && svg.contains("fill=\"#ffffff\"") && svg.contains("fill=\"#000000\"") && svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<path").count(), 1);
        assert!(svg.contains("M4 4h7v1h-7z"), "the first row starts with the finder pattern's seven dark modules at the quiet zone");
        // PNG: signature, size, and the pixels scan
        let png = qrcode::png(&q).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let pm = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!(pm.width(), pm.height());
        assert_eq!(pm.width() as usize % n, 0, "a whole number of pixels to a module");
        assert!(pm.width() >= 4 * n as u32);
        assert_eq!(pm.pixel(0, 0).unwrap().red(), 255, "the quiet zone is white");
        let got = iyakki_agent::qr_decode::decode_rgba(pm.width() as usize, pm.height() as usize, pm.data());
        assert_eq!(got.as_deref(), Some(text.as_bytes()), "level {level:?}");
    }
    // a big symbol still gets at least four pixels to a module
    let big = iyakki_agent::qr::encode_level(&vec![b'q'; 2000], Level::L).unwrap();
    assert!(qrcode::png(&big).is_ok());
    assert!(qrcode::file_name("png").starts_with("qr-") && qrcode::file_name("svg").ends_with(".svg"));
}

#[test]
fn the_page_registers_its_buttons_and_draws_the_code_at_several_sizes() {
    for (w, h, s) in [(1280, 800, 1.0), (800, 480, 1.0), (900, 600, 1.0), (360, 740, 1.0), (360, 360, 1.0), (1600, 1000, 1.6)] {
        let mut f = Fx::new(w, h, s);
        f.click(Event::Tab(T_TOOL + tools::QR));
        assert_eq!(f.app.page(), "qr-code");
        f.ev(Event::Paste("https://iyakki.test/hello".into()));
        let t = f.text();
        let wf = w as f32;
        let kinds: Vec<usize> = if f.app.short { vec![T_QR_NEXT] } else { (0..PRESETS.len()).map(|i| T_QR_KIND + i).collect() };
        let ids: Vec<usize> = kinds.into_iter().chain([T_QR_LEVEL, T_COPY, T_SAVE_PNG, T_SAVE_SVG, T_PASTE, T_CLEAR]).collect();
        for id in ids {
            let r = f.hit(&Event::Tab(id)).unwrap_or_else(|| panic!("{w}x{h}: button {id} is not drawn:\n{t}"));
            assert!(r.w > 0.0 && r.h > 0.0 && r.x >= 0.0 && r.x + r.w <= wf, "{w}x{h}: button {id} at {r:?}");
            if h >= 480 {
                assert!(r.y + r.h <= h as f32, "{w}x{h}: button {id} at {r:?}");
            }
        }
        assert!(f.hit(&Event::Area(A_OUT)).is_some(), "{w}x{h}: the frame of the code");
        if w >= 800 {
            assert!(t.contains("Version 2") && t.contains("level M") && t.contains("bytes used"), "{w}x{h}:\n{t}");
            assert!(t.contains("Error correction: M") && t.contains("Save as PNG") && t.contains("Save as SVG") && t.contains("Copy text") && t.contains("Clear"), "{w}x{h}:\n{t}");
        }
        // a white square with black modules is on the screen, whatever the theme (the page was cleared dark)
        let (white, black) = f.count_pixels();
        assert!(white > 500 && black > 100, "{w}x{h}: {white} white and {black} black pixels");
    }
}

#[test]
fn the_drawn_code_scans_back_from_the_screen() {
    for (w, h, s) in [(1280, 800, 1.0), (900, 600, 1.0), (1600, 1000, 1.6)] {
        let mut f = Fx::new(w, h, s);
        f.click(Event::Tab(T_TOOL + tools::QR));
        f.ev(Event::Paste("https://iyakki.test/scan-me".into()));
        let got = iyakki_agent::qr_decode::decode_rgba(w as usize, h as usize, f.g.rgba());
        assert_eq!(got.as_deref(), Some("https://iyakki.test/scan-me".as_bytes()), "{w}x{h}");
    }
}

#[test]
fn a_wifi_code_says_the_password_is_inside_and_open_networks_do_not() {
    let mut f = Fx::new(1000, 700, 1.0);
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.click(Event::Tab(T_QR_KIND + qrcode::WIFI));
    f.ev(Event::Paste("iyakki\nsecret-pass".into()));
    assert!(f.text().contains("holds the Wi-Fi password in plain form"), "{}", f.text());
    assert!(f.text().contains("Security: WPA") && f.text().contains("Hidden network: no"), "{}", f.text());
    assert_eq!(f.app.qr.made("iyakki\nsecret-pass").payload, "WIFI:T:WPA;S:iyakki;P:secret-pass;;");
    f.click(Event::Tab(T_QR_HIDDEN));
    f.click(Event::Tab(T_QR_SEC));
    assert!(f.text().contains("Security: WEP") && f.text().contains("Hidden network: yes"));
    assert_eq!(f.app.qr.made("iyakki\nsecret-pass").payload, "WIFI:T:WEP;S:iyakki;P:secret-pass;H:true;;");
    f.click(Event::Tab(T_QR_SEC));
    assert!(!f.text().contains("holds the Wi-Fi password"), "an open network has no password to warn about");
    // the other kinds show neither the choice nor the warning
    f.click(Event::Tab(T_QR_KIND));
    assert!(f.hit(&Event::Tab(T_QR_SEC)).is_none() && !f.text().contains("Wi-Fi password"));
}

#[test]
fn kinds_keep_their_own_text_and_the_level_changes_the_code() {
    let mut f = Fx::new(1000, 700, 1.0);
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.ev(Event::Paste("some note".into()));
    f.click(Event::Tab(T_QR_KIND + 1));
    assert_eq!(f.app.input.text, "", "a new kind starts empty");
    f.ev(Event::Paste("iyakki.test".into()));
    assert_eq!(f.app.qr.made("iyakki.test").payload, "https://iyakki.test");
    f.click(Event::Tab(T_QR_KIND));
    assert_eq!(f.app.input.text, "some note");
    f.click(Event::Tab(T_QR_KIND + 1));
    assert_eq!(f.app.input.text, "iyakki.test");
    // the level cycles M, Q, H, L, M and a higher level never makes a smaller code
    let mut sizes = vec![];
    for want in ['Q', 'H', 'L', 'M'] {
        f.click(Event::Tab(T_QR_LEVEL));
        assert!(f.text().contains(&format!("Error correction: {want}")), "{}", f.text());
        sizes.push((want, f.app.qr.made("iyakki.test").qr.as_ref().unwrap().size));
    }
    assert!(sizes[2].1 <= sizes[3].1 && sizes[3].1 <= sizes[0].1 && sizes[0].1 <= sizes[1].1, "L, M, Q, H: {sizes:?}");
}

#[test]
fn the_keyboard_alone_picks_a_kind_types_and_copies() {
    let mut f = Fx::new(1000, 700, 1.0);
    f.click(Event::Tab(T_TOOL + tools::QR));
    assert_eq!(f.app.focus, Focus::Input);
    for c in "hello".chars() {
        f.key(Key::Char(c));
    }
    assert_eq!(f.app.input.text, "hello");
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Buttons);
    f.key(Key::Right);
    f.key(Key::Enter);
    assert_eq!(f.app.qr.preset, 1, "Right then Enter chose the URL kind");
    f.key(Key::Left);
    f.key(Key::Enter);
    assert_eq!(f.app.qr.preset, 0);
    assert_eq!(f.app.input.text, "hello");
    // Copy text with the keyboard: walk to it
    let at = f.app.buttons().iter().position(|(_, id)| *id == T_COPY).unwrap();
    for _ in 0..at {
        f.key(Key::Right);
    }
    f.key(Key::Enter);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("hello"));
    assert!(f.text().contains("Copied the text of the code."), "{}", f.text());
    // Ctrl+Enter from the box does the same
    crate::ui::clipboard::copy("other");
    f.key(Key::Esc);
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Input);
    f.key(Key::Ctrl('\n'));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("hello"));
}

#[test]
fn clear_empties_every_kind_and_copy_has_nothing_to_copy_when_empty() {
    let mut f = Fx::new(1000, 700, 1.0);
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.ev(Event::Paste("abc".into()));
    f.click(Event::Tab(T_QR_KIND + 3));
    f.ev(Event::Paste("+49 30 1234567".into()));
    f.click(Event::Tab(T_CLEAR));
    assert!(f.app.input.text.is_empty());
    f.click(Event::Tab(T_QR_KIND));
    assert!(f.app.input.text.is_empty(), "the text kept for the Text kind went too");
    crate::ui::clipboard::copy("before");
    f.click(Event::Tab(T_COPY));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("before"));
    assert!(f.text().contains("Type something above"), "{}", f.text());
}

#[test]
fn too_long_text_is_said_in_plain_words_and_nothing_is_saved() {
    let mut f = Fx::new(1000, 700, 1.0);
    let home = crate::apps::tests::temp_dir("devtools-qr-long");
    f.app.qr_home = Some(home.clone());
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.app.input.set(&"y".repeat(3000));
    f.draw();
    assert!(f.text().contains("Too long for a QR code"), "{}", f.text());
    f.click(Event::Tab(T_SAVE_PNG));
    assert!(f.app.qr.note.as_ref().is_some_and(|(t, bad)| *bad && t.contains("Too long")), "{:?}", f.app.qr.note);
    assert!(!home.join("Pictures").exists() || std::fs::read_dir(home.join("Pictures")).unwrap().next().is_none());
    // nothing typed: a plain sentence too
    f.click(Event::Tab(T_CLEAR));
    f.click(Event::Tab(T_SAVE_SVG));
    assert!(f.app.qr.note.as_ref().is_some_and(|(t, bad)| *bad && t.contains("no code to save yet")), "{:?}", f.app.qr.note);
}

#[test]
fn save_writes_a_valid_png_and_svg_into_the_pictures_folder_and_says_where() {
    let mut f = Fx::new(1000, 700, 1.0);
    let home = crate::apps::tests::temp_dir("devtools-qr-save");
    f.app.qr_home = Some(home.clone());
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.ev(Event::Paste("https://iyakki.test/saved".into()));
    f.click(Event::Tab(T_SAVE_PNG));
    let (note, bad) = f.app.qr.note.clone().unwrap();
    assert!(!bad && note.starts_with("Saved qr-") && note.contains(".png in ") && note.contains("Pictures"), "{note}");
    assert!(f.text().contains("Saved qr-"), "the page shows where it went");
    f.click(Event::Tab(T_SAVE_SVG));
    let files: Vec<_> = std::fs::read_dir(home.join("Pictures")).unwrap().map(|e| e.unwrap().path()).collect();
    let png = files.iter().find(|p| p.extension().is_some_and(|e| e == "png")).expect("a PNG");
    let svg = files.iter().find(|p| p.extension().is_some_and(|e| e == "svg")).expect("an SVG");
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("qr-20") && name.len() > 18, "named by the time: {name}");
    }
    let bytes = std::fs::read(png).unwrap();
    let pm = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
    assert_eq!(iyakki_agent::qr_decode::decode_rgba(pm.width() as usize, pm.height() as usize, pm.data()).as_deref(), Some("https://iyakki.test/saved".as_bytes()));
    let text = std::fs::read_to_string(svg).unwrap();
    assert!(text.starts_with("<?xml") && text.contains("<svg ") && text.contains("</svg>"));
    // saving again the same second never overwrites
    f.click(Event::Tab(T_SAVE_PNG));
    f.click(Event::Tab(T_SAVE_PNG));
    assert!(std::fs::read_dir(home.join("Pictures")).unwrap().count() >= 4);
    // the log has the file, not what the code holds
    let log = f.cx.audit.recent(20).join("\n");
    assert!(log.contains("devtools.qr.save") && !log.contains("iyakki.test/saved"), "{log}");
}

#[test]
fn a_folder_the_account_may_not_write_gives_a_sentence_not_a_crash() {
    let mut f = Fx::new(1000, 700, 1.0);
    f.cx.session.policy.fs_write = vec!["/nowhere".into()];
    f.app.qr_home = Some(crate::apps::tests::temp_dir("devtools-qr-denied"));
    f.click(Event::Tab(T_TOOL + tools::QR));
    f.ev(Event::Paste("text".into()));
    f.click(Event::Tab(T_SAVE_PNG));
    assert!(f.app.qr.note.as_ref().is_some_and(|(t, bad)| *bad && t.starts_with("Could not save the picture")), "{:?}", f.app.qr.note);
}

#[test]
fn a_short_window_has_one_button_that_goes_to_the_next_kind() {
    let mut f = Fx::new(360, 360, 1.0);
    f.click(Event::Tab(T_TOOL + tools::QR));
    assert!(f.app.short && f.hit(&Event::Tab(T_QR_KIND + 1)).is_none());
    assert!(f.text().contains("Kind: Text"), "{}", f.text());
    for want in 1..PRESETS.len() {
        f.click(Event::Tab(T_QR_NEXT));
        assert_eq!(f.app.qr.preset, want);
        assert!(f.text().contains(&format!("Kind: {}", PRESETS[want].name)), "{}", f.text());
    }
    f.click(Event::Tab(T_QR_NEXT));
    assert_eq!(f.app.qr.preset, 0, "after the last it starts again");
    // the code still fits the window (it scrolls inside its frame when it must)
    f.ev(Event::Paste("hi".into()));
    let (white, black) = f.count_pixels();
    assert!(white > 200 && black > 50, "{white} white, {black} black");
}
