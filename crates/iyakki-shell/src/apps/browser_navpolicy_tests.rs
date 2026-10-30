// What the tab does with the addresses the engine reports (browser_full.rs `classify_url`): pages without a host are no reason to
// refuse or to leave the full engine. Included (include!) into the browser's test module.

#[test]
fn addresses_without_a_host_are_not_refused_and_real_hosts_are_vetted() {
    use crate::apps::browser_full::{classify_url, UrlVerdict as V};
    let only_a = |h: &str| h == "a.org";
    for ok in ["about:blank", "about:srcdoc", "data:text/html,hi", "blob:https://a.org/1-2", "https://a.org/x"] {
        assert_eq!(classify_url(ok, &only_a), V::Pass, "{ok}");
    }
    for ignored in ["", "   ", "javascript:void(0)", "JavaScript:alert(1)"] {
        assert_eq!(classify_url(ignored, &only_a), V::Ignore, "{ignored:?}");
    }
    assert_eq!(classify_url("https://evil.example/x", &only_a), V::Refuse("evil.example".into()));
    assert_eq!(classify_url("file:///etc/hosts", &only_a), V::NoAddress("file".into()));
    assert_eq!(classify_url("https:///nohost", &only_a), V::NoAddress("https".into()));
}

fn settle_events(a: &mut BrowserApp, cx: &AppCx) {
    for _ in 0..60 {
        a.handle(&Event::Tick, cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn blank_data_blob_srcdoc_empty_and_script_addresses_from_the_engine_keep_the_tab_and_raise_no_refusal() {
    let (_fake, cx, mut a, _g) = in_full("navpol1", "--static");
    let mut limited = cx.clone();
    limited.session.policy.net_hosts = vec!["a.org".into()];
    for addr in ["about:blank", "data:text/html,hi", "blob:https://a.org/1", "about:srcdoc", "", "javascript:void(0)"] {
        a.full.as_mut().unwrap().navigate(&format!("fake://report/{addr}"));
        settle_events(&mut a, &limited);
        assert!(a.full.is_some(), "{addr:?}: the tab stays in the full engine");
        let st = a.status_text().map(|s| s.to_string()).unwrap_or_default();
        assert!(!st.contains("may not open") && !st.contains("went back"), "{addr:?}: {st}");
    }
    let audit = std::fs::read_to_string(cx.services.dir().parent().unwrap().join("audit.log")).unwrap_or_default();
    assert!(!audit.contains("browser.full.refused"), "{audit}");
}

#[test]
fn a_top_level_address_with_no_host_gets_a_clear_message_and_the_tab_is_not_dropped() {
    let (_fake, cx, mut a, _g) = in_full("navpol2", "--static");
    a.full.as_mut().unwrap().navigate("fake://report/file:///etc/hosts");
    pump(&mut a, &cx, "the message", |a| a.status_text().is_some_and(|s| s.contains("no web address")));
    let st = a.status_text().unwrap().to_string();
    assert!(st.contains("file:") && !st.contains("may not open"), "{st}");
    assert!(a.full.is_some() && a.full.as_ref().unwrap().url == "https://a.org/", "the tab keeps its page");
}

#[test]
fn a_refused_real_host_names_the_host_and_who_decides_and_the_tab_stays() {
    let (_fake, cx, mut a, _g) = in_full("navpol3", "--static");
    let mut limited = cx.clone();
    limited.session.policy.net_hosts = vec!["a.org".into()];
    a.full.as_mut().unwrap().navigate("fake://report/https://evil.example/x");
    pump(&mut a, &limited, "the refusal", |a| a.status_text().is_some_and(|s| s.contains("evil.example")));
    let st = a.status_text().unwrap().to_string();
    assert!(st.contains("may not open pages from evil.example") && st.contains("administrator") && st.contains("went back"), "{st}");
    assert!(a.full.is_some(), "the engine tab is not killed");
    assert!(!a.data.history.iter().any(|v| v.url.contains("evil.example")));
}
