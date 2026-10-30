//! Tests for the HYBRID filters. Names are example.com style, addresses are documentation ranges.

use crate::filter::*;
use crate::net::{default_cluster_cidrs, Cidr, Context};
use crate::VpnError;

fn set(t: &str) -> FilterSet {
    FilterSet::parse(t).unwrap_or_else(|e| panic!("{t:?}: {e}"))
}
fn url(f: &FilterSet, u: &str) -> Route {
    f.decide_url(u).unwrap().route
}
fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![Cidr::parse("192.168.1.0/24").unwrap()], tunnel: vec![Cidr::parse("10.8.0.2/24").unwrap()] }
}

#[test]
fn entries_are_parsed_normalised_and_listed_back() {
    let f = set("# comment\n\nExample.COM\n*.Example.com   # trailing note\n!mail.example.com\nhttps://App.Example.com:8443/Api/v1/\nexample.org/docs\n203.0.113.9\n203.0.113.77/24\n2001:db8::/32\nexample.com\r\n");
    assert_eq!(
        f.entries(),
        ["example.com", "*.example.com", "!mail.example.com", "https://app.example.com:8443/Api/v1", "example.org/docs", "203.0.113.9/32", "203.0.113.0/24", "2001:db8::/32"]
    );
    // what is stored parses back to the same set
    assert_eq!(FilterSet::from_entries(&f.entries()).unwrap(), f);
    let json = serde_json::to_string(&f).unwrap();
    assert_eq!(serde_json::from_str::<FilterSet>(&json).unwrap(), f);
    assert!(serde_json::from_str::<FilterSet>("[\"*.com\"]").is_err());
    assert_eq!(f.dns_ignored_url_rules(), 2);
    assert_eq!(f.nets().len(), 3);
}

#[test]
fn catch_alls_are_refused() {
    for t in [
        "*", "*.", "*.*", "*.com", "*.org", "*.co.uk", "*.com.au", "*.github.io", "*.amazonaws.com", "!*", "*example.com", "ex*.example.com", "www.*.com", "**.example.com", "*.*.example.com", "https://*.com/", "https://*/",
        "0.0.0.0/0", "::/0", "0.0.0.0/1", "128.0.0.0/1", "10.0.0.0/7", "2000::/3", "8000::/1", "0.0.0.0", "!0.0.0.0/0",
    ] {
        let r = FilterSet::parse(t);
        match t {
            // a single address is fine; the all-zero host is a valid /32 (and checked against the network later)
            "0.0.0.0" => assert!(r.is_ok(), "{t}"),
            _ => assert!(r.is_err(), "{t}: {r:?}"),
        }
    }
    // narrow enough is fine
    for t in ["*.example.com", "*.example.co.uk", "*.sub.github.io", "10.0.0.0/8", "2001:db8::/16", "*.a.b.c.example.com"] {
        assert!(FilterSet::parse(t).is_ok(), "{t}");
    }
}

#[test]
fn hostile_and_odd_entries_are_refused() {
    for t in [
        "exa mple.com", "example.com;reboot", "$(id).example.com", "`id`.com", "-bad.example.com", "bad-.example.com", "a..b.com", ".example.com", "example.com.:80", "exämple.com", "ex\u{202e}ample.com", "localhost", "foo.localhost",
        "1.2.3", "999.1.1.1", "1.2.3.4/33", "::g/64", "http://", "http:///x", "ftp://example.com/x", "file:///etc/passwd", "javascript:alert(1)//x", "https://user@example.com/", "https://example.com/a?b=c", "https://example.com/#x",
        "https://example.com/../x", "https://example.com/a/./b", "https://example.com/%2e%2e/x", "https://example.com/a\\b", "https://example.com:0/", "https://example.com:99999/", "https://example.com:80x/", "https://[::1/",
        "https://example.com/*", "https://example.com/a*", "example_x.com", "https://exa_mple.com/",
    ] {
        assert!(FilterSet::parse(t).is_err(), "{t:?}");
    }
    for c in ["\0", "\u{1b}", "\u{7f}", "\u{8}", "\u{0b}"] {
        assert!(matches!(FilterSet::parse(&format!("example.com{c}")), Err(VpnError::BadText(_))), "{c:?}");
    }
    assert!(matches!(FilterSet::parse(&"a".repeat(400)), Err(VpnError::TooLarge(_) | VpnError::Invalid { .. })));
    assert!(matches!(FilterSet::parse(&format!("{}.com", "a".repeat(300))), Err(VpnError::TooLarge(_))));
    assert!(FilterSet::parse(&format!("{}.com", "a".repeat(64))).is_err()); // a label over 63 bytes
}

#[test]
fn limits_on_count_and_size() {
    let ok: String = (0..MAX_RULES).map(|i| format!("h{i}.example.com\n")).collect();
    assert_eq!(set(&ok).len(), MAX_RULES);
    let too_many = format!("{ok}one-more.example.com\n");
    assert!(matches!(FilterSet::parse(&too_many), Err(VpnError::TooLarge(_))));
    // duplicates do not count twice
    let dups = "a.example.com\n".repeat(10_000);
    assert!(matches!(FilterSet::parse(&dups), Err(VpnError::TooLarge(_))), "the text itself is over the size limit");
    assert_eq!(set(&"a.example.com\n".repeat(1000)).len(), 1);
    assert!(matches!(FilterSet::parse(&"#".repeat(MAX_TEXT + 1)), Err(VpnError::TooLarge(_))));
    assert!(FilterSet::from_entries(&vec!["x.example.com".to_string(); 10]).is_ok());
    let huge: Vec<String> = (0..2000).map(|i| format!("h{i}.example.com")).collect();
    assert!(FilterSet::from_entries(&huge).is_err());
    // a deep path
    let deep = format!("example.com/{}", "a/".repeat(MAX_PATH_SEGMENTS + 1));
    assert!(FilterSet::parse(&deep).is_err());
    assert!(FilterSet::parse(&format!("example.com/{}", "a/".repeat(MAX_PATH_SEGMENTS))).is_ok());
    assert!(FilterSet::new().is_empty() && !FilterSet::new().has_includes());
}

#[test]
fn domain_rules_match_exactly_and_wildcards_match_only_subdomains() {
    let f = set("example.com\n*.cdn.example.net");
    assert_eq!(f.decide_domain("example.com").route, Route::Tunnel);
    assert_eq!(f.decide_domain("EXAMPLE.com.").route, Route::Tunnel);
    assert_eq!(f.decide_domain("www.example.com").route, Route::Direct);
    assert_eq!(f.decide_domain("notexample.com").route, Route::Direct);
    assert_eq!(f.decide_domain("a.cdn.example.net").route, Route::Tunnel);
    assert_eq!(f.decide_domain("x.y.cdn.example.net").route, Route::Tunnel);
    assert_eq!(f.decide_domain("cdn.example.net").route, Route::Direct, "a wildcard does not cover its own apex");
    assert_eq!(f.decide_domain("evilcdn.example.net").route, Route::Direct);
    assert_eq!(f.decide_domain("cdn.example.net.evil.com").route, Route::Direct);
    for odd in ["", ".", "..", "a b.com", &"a".repeat(300), "exa\u{0}mple.com", "?.com"] {
        assert_eq!(f.decide_domain(odd).route, Route::Direct, "{odd:?}");
    }
}

#[test]
fn most_specific_wins_and_an_exclusion_wins_a_tie() {
    let f = set("*.example.com\n!mail.example.com\nlogin.example.com\n!*.internal.example.com\n*.vpn.internal.example.com");
    assert_eq!(f.decide_domain("www.example.com").route, Route::Tunnel);
    assert_eq!(f.decide_domain("mail.example.com").route, Route::Direct);
    assert_eq!(f.decide_domain("login.example.com").route, Route::Tunnel);
    assert_eq!(f.decide_domain("a.internal.example.com").route, Route::Direct);
    assert_eq!(f.decide_domain("x.vpn.internal.example.com").route, Route::Tunnel, "a longer wildcard is more specific than the exclusion");
    // an exact include under an exclusion of the same exact name: the exclusion wins the tie
    let t = set("a.example.com\n!a.example.com");
    assert_eq!(t.decide_domain("a.example.com").route, Route::Direct);
    let t = set("!a.example.com\na.example.com");
    assert_eq!(t.decide_domain("a.example.com").route, Route::Direct, "order of the lines does not matter");
    // an exact include beats an exclusion wildcard; the reverse holds too
    assert_eq!(set("!*.example.com\nwww.example.com").decide_domain("www.example.com").route, Route::Tunnel);
    assert_eq!(set("*.example.com\n!www.example.com").decide_domain("www.example.com").route, Route::Direct);
}

#[test]
fn addresses_by_prefix_length() {
    let f = set("203.0.113.0/24\n!203.0.113.128/25\n203.0.113.200/32\n2001:db8::/32\n!2001:db8:bad::/48");
    let r = |s: &str| f.decide_ip(s.parse().unwrap()).route;
    assert_eq!(r("203.0.113.5"), Route::Tunnel);
    assert_eq!(r("203.0.113.130"), Route::Direct);
    assert_eq!(r("203.0.113.200"), Route::Tunnel);
    assert_eq!(r("203.0.114.1"), Route::Direct);
    assert_eq!(r("2001:db8:1::1"), Route::Tunnel);
    assert_eq!(r("2001:db8:bad::1"), Route::Direct);
    assert_eq!(r("::ffff:203.0.113.5"), Route::Tunnel, "an IPv4-mapped IPv6 address is judged as the IPv4 address");
    // domains and URL rules do not apply to bare addresses
    assert_eq!(set("example.com").decide_ip("203.0.113.5".parse().unwrap()).route, Route::Direct);
}

#[test]
fn url_rules_match_scheme_host_port_and_whole_path_segments() {
    let f = set("https://example.com/app\nhttp://example.com:8080/\n*.api.example.net/v2\nexample.org\n!https://example.com/app/public");
    assert_eq!(url(&f, "https://example.com/app"), Route::Tunnel);
    assert_eq!(url(&f, "https://example.com/app/"), Route::Tunnel);
    assert_eq!(url(&f, "https://example.com/app/x/y?q=1#f"), Route::Tunnel);
    assert_eq!(url(&f, "https://EXAMPLE.com:443/app/x"), Route::Tunnel, "default port and case are normalised");
    assert_eq!(url(&f, "https://example.com/apple"), Route::Direct, "a path matches whole segments only");
    assert_eq!(url(&f, "https://example.com/"), Route::Direct);
    assert_eq!(url(&f, "http://example.com/app"), Route::Direct, "the scheme is part of the rule");
    assert_eq!(url(&f, "https://example.com/app/public/x"), Route::Direct, "the exclusion is more specific");
    assert_eq!(url(&f, "http://example.com:8080/anything"), Route::Tunnel);
    assert_eq!(url(&f, "http://example.com:8081/anything"), Route::Direct);
    assert_eq!(url(&f, "https://a.api.example.net/v2/users"), Route::Tunnel);
    assert_eq!(url(&f, "wss://a.api.example.net/v2/socket"), Route::Tunnel, "no scheme in the rule: any of http, https, ws, wss");
    assert_eq!(url(&f, "https://a.api.example.net/v1/users"), Route::Direct);
    assert_eq!(url(&f, "https://api.example.net/v2/users"), Route::Direct);
    // a plain domain rule applies to every URL of that host
    assert_eq!(url(&f, "ftp://example.org/x"), Route::Tunnel);
    assert_eq!(url(&f, "https://example.org:9999/any/path"), Route::Tunnel);
}

#[test]
fn path_tricks_do_not_slip_a_url_past_its_rule() {
    let f = set("https://example.com/private");
    for u in [
        "https://example.com/private/../private/x",
        "https://example.com/public/../private",
        "https://example.com/%70rivate",
        "https://example.com/%50rivate/x".replace("%50", "%70").as_str(),
        "https://example.com/./private",
        "https://example.com//../private",
        "https://example.com/private/.",
        "https://example.com\\private",
        "https://example.com/x/..\\private",
    ] {
        // \ after the host ends the authority in browsers; such URLs are refused or judged as the normalised path
        match f.decide_url(u) {
            Ok(d) => assert_eq!(d.route, Route::Tunnel, "{u}"),
            Err(_) => {}
        }
    }
    assert_eq!(url(&f, "https://example.com/public/../private"), Route::Tunnel);
    assert_eq!(url(&f, "https://example.com/private/../public"), Route::Direct);
    assert_eq!(url(&f, "https://example.com/%70rivate"), Route::Tunnel);
    assert_eq!(url(&f, "https://example.com/private/x/../.."), Route::Direct);
    // an encoded slash is ambiguous: the URL is refused, not routed direct
    assert!(f.decide_url("https://example.com/private%2Fx").is_err());
    assert!(f.decide_url("https://example.com/private%5Cx").is_err());
    assert!(f.decide_url("https://example.com/%00").is_err());
    assert!(f.decide_url("https://example.com/%zz").is_err());
}

#[test]
fn urls_that_cannot_be_judged_are_errors_so_the_caller_does_not_connect() {
    let f = set("example.com");
    for u in [
        "", "example.com", "https://", "https:///x", "https://user@example.com/", "https://example.com@evil.com/", "https://user:pw@example.com/", "https://exämple.com/", "https://ex ample.com/", "https://example.com/a b",
        "https://example.com:0/", "https://example.com:70000/", "https://0x7f.1/", "https://2130706433/", "https://127.1/", "https://1.2.3/", "https://01.2.3.4/", "file:///etc/passwd", "data:text/plain,hi", "javascript:alert(1)", "gopher://example.com/",
        "https://[::1/", "https://exa\tmple.com/", "https://example.com/\n", "https://ex_ample.com/",
    ] {
        assert!(f.decide_url(u).is_err(), "{u:?}");
    }
    let long = format!("https://example.com/{}", "a".repeat(MAX_URL));
    assert!(f.decide_url(&long).is_err());
    // an address host is judged by the address rules
    let g = set("203.0.113.0/24");
    assert_eq!(url(&g, "http://203.0.113.9/x"), Route::Tunnel);
    assert_eq!(url(&g, "http://[2001:db8::1]/x"), Route::Direct);
    assert_eq!(url(&g, "http://8.8.8.8/x"), Route::Direct);
}

#[test]
fn every_decision_comes_with_a_sentence() {
    let f = set("*.example.com\n!mail.example.com\nhttps://example.org/app");
    let d = f.decide_domain("www.example.com");
    assert_eq!(d.explain(), "www.example.com goes through the VPN because the rule \"*.example.com\" matches it.");
    let d = f.decide_domain("mail.example.com");
    assert!(d.explain().contains("goes direct because the exclusion \"!mail.example.com\" matches it"));
    assert!(d.explain().contains("\"*.example.com\" also matches"));
    let d = f.decide_domain("example.net");
    assert_eq!(d.explain(), "example.net goes direct: no rule of this profile matches it.");
    let d = f.decide_url("https://example.org/app/x").unwrap();
    assert!(d.explain().contains("https://example.org/app") && d.route == Route::Tunnel);
    assert!(d.explain().starts_with("https://example.org/app/x goes through the VPN"));
    // FULL mode
    let full = decide_url_for(true, &FilterSet::new(), "https://anything.example/x").unwrap();
    assert_eq!(full.route, Route::Tunnel);
    assert!(full.explain().contains("all traffic"));
    assert!(decide_url_for(true, &FilterSet::new(), "garbage").is_err());
    assert_eq!(decide_url_for(false, &f, "https://www.example.com/").unwrap().route, Route::Tunnel);
    // the test box
    assert_eq!(decide_any(&f, "www.example.com").unwrap().route, Route::Tunnel);
    assert_eq!(decide_any(&f, " https://example.org/app ").unwrap().route, Route::Tunnel);
    assert_eq!(decide_any(&f, "203.0.113.5").unwrap().route, Route::Direct);
    assert!(decide_any(&f, "not a thing!").is_err());
    // long hostile subjects are cut in the sentence
    let long = "x".repeat(250) + ".example.net";
    assert!(f.decide_domain(&"é".repeat(100)).subject.chars().count() <= 80);
    let _ = long;
}

#[test]
fn what_the_dns_layer_cannot_see_is_counted_and_reported() {
    let f = set("example.com\nhttps://example.org/app\nexample.net/x");
    assert_eq!(f.dns_ignored_url_rules(), 2);
    // a URL rule does not make the host a tunnel name for other programs
    assert_eq!(f.decide_domain("example.org").route, Route::Direct);
    assert_eq!(f.decide_domain("example.net").route, Route::Direct);
    let w = f.check_routes(&ctx()).unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("only in IYAKKI's own browser"));
}

#[test]
fn address_rules_are_checked_against_the_real_network() {
    let x = ctx();
    for t in ["10.42.0.0/24", "10.43.0.0/16", "127.0.0.0/8", "169.254.169.254", "192.168.1.0/24", "192.168.0.0/16", "10.0.0.0/8"] {
        assert!(matches!(set(t).check_routes(&x), Err(VpnError::Unsafe(_))), "{t}");
        // the same range as an exclusion is harmless
        assert!(set(&format!("!{t}")).check_routes(&x).is_ok(), "!{t}");
    }
    assert_eq!(set("10.8.0.0/24").check_routes(&x).unwrap().len(), 0);
    assert_eq!(set("172.20.0.0/16").check_routes(&x).unwrap().len(), 1);
    assert!(set("203.0.113.0/24").check_routes(&x).unwrap().is_empty());
}
