// The lock in the address bar opens the certificate of the open page's site (F-BROW-98). Included (include!) into the browser's test module.

fn fake_view() -> iyakki_proxy::certview::View {
    iyakki_proxy::certview::View { host: "a.org".into(), subject: "CN=a.org".into(), issuer: "CN=Some Authority".into(), not_before: "2026-01-01".into(), not_after: "2027-01-01".into(), days_left: 90, sha256: "AA:BB".into(), names_match: true, trust: Err("UnknownIssuer".into()), sent: 1, self_issued: false, warnings: vec!["It does not lead to a trusted authority (UnknownIssuer).".into()] }
}

#[test]
fn the_lock_and_k_show_the_certificate_of_the_open_page_and_a_second_press_closes_it() {
    let _turn = crate::apps::chat_settings::CERT_FAKE_TURN.lock().unwrap_or_else(|e| e.into_inner());
    let web = site(vec![("https://a.org/", html(GUIDE))]);
    let cx = cx_with("brcert1", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("a.org", &cx);
    settle(&mut a, &cx);
    assert!(a.cert.is_none());
    *crate::apps::chat_settings::CERT_FAKE.lock().unwrap() = Some(fake_view());
    key(&mut a, &cx, Key::Ctrl('u'));
    let panel = a.cert.as_ref().expect("the panel is open");
    panel.wait();
    *crate::apps::chat_settings::CERT_FAKE.lock().unwrap() = None;
    let lines = panel.lines();
    assert!(lines.iter().any(|l| l.contains("Issued by: CN=Some Authority")) && lines.iter().any(|l| l.contains("Trusted authority: NO")), "{lines:?}");
    let _ = frame(&mut a, &cx, 1280, 720);
    let _ = frame(&mut a, &cx, 360, 740);
    // a second Ctrl+U closes it, and a click on the lock opens it again (the lock is the hit region the bar draws)
    key(&mut a, &cx, Key::Ctrl('u'));
    assert!(a.cert.is_none());
    a.handle(&Event::Tab(crate::apps::browser_certview::TAB_CERT), &cx);
    assert!(a.cert.is_some(), "the lock opens the panel");
    a.cert.as_ref().unwrap().wait();
    a.handle(&Event::Tab(crate::apps::browser_certview::TAB_CERT), &cx);
    assert!(a.cert.is_none());
}

#[test]
fn a_page_that_is_not_secure_has_no_certificate_and_a_forbidden_host_is_never_asked() {
    let _turn = crate::apps::chat_settings::CERT_FAKE_TURN.lock().unwrap_or_else(|e| e.into_inner());
    let web = site(vec![("http://plain.org/", html(GUIDE))]);
    let cx = cx_with("brcert2", web);
    let mut a = BrowserApp::new(&cx);
    a.open_url("http://plain.org/", &cx);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::Ctrl('u'));
    let p = a.cert.as_ref().expect("the panel says why there is nothing");
    assert!(p.done(), "nothing was asked: no handshake for a plain page");
    assert!(p.lines()[0].contains("not secure"), "{:?}", p.lines());
    let _ = frame(&mut a, &cx, 1280, 720);
    // nothing opens without a page
    let mut b = BrowserApp::new(&cx);
    key(&mut b, &cx, Key::Ctrl('u'));
    assert!(b.cert.is_none());
}
