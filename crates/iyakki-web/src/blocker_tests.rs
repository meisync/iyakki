use super::*;
use crate::blocker_json;
use crate::blocker_lists::{sha256_hex, ListMeta, ListSource, ListStore};
use crate::blocker_state::{interstitial_html, Filter, FilteredFetch, INTERSTITIAL_TITLE};
use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

fn bl(text: &str) -> Blocker {
    compile_lists([text])
}

fn blocked(b: &Blocker, url: &str, page: &str, kind: ResType) -> bool {
    b.is_blocked(url, page, kind)
}

#[test]
fn domain_anchor_blocks_the_domain_and_its_subdomains_only() {
    let b = bl("||ads.example.com^\n");
    for u in ["https://ads.example.com/x.js", "http://ads.example.com", "https://a.b.ads.example.com/p?q=1", "https://ads.example.com:8080/p", "https://user@ads.example.com/"] {
        assert!(blocked(&b, u, "https://news.org/", ResType::Script), "{u}");
    }
    for u in ["https://example.com/ads.example.com/", "https://notads.example.com/", "https://ads.example.com.evil.org/x", "https://example.com/", "https://xads.example.com/"] {
        assert!(!blocked(&b, u, "https://news.org/", ResType::Script), "{u}");
    }
}

#[test]
fn start_end_anchors_wildcards_and_separators() {
    let b = bl("|https://tracker.net/p\n/banner/*/img^\nswf|\n||cdn.org/a*b^c\n");
    assert!(blocked(&b, "https://tracker.net/pixel", "", ResType::Image));
    assert!(!blocked(&b, "http://tracker.net/pixel", "", ResType::Image), "| anchors at the start");
    assert!(!blocked(&b, "https://x.org/?u=https://tracker.net/p", "", ResType::Image));
    assert!(blocked(&b, "https://x.org/banner/300x250/img?x=1", "", ResType::Image));
    assert!(blocked(&b, "https://x.org/banner/a/b/img", "", ResType::Image), "^ matches the end");
    assert!(!blocked(&b, "https://x.org/banner/300x250/imgs", "", ResType::Image), "^ needs a separator or the end");
    assert!(blocked(&b, "https://x.org/movie.swf", "", ResType::Other));
    assert!(!blocked(&b, "https://x.org/movie.swf?x", "", ResType::Other), "trailing | anchors at the end");
    assert!(blocked(&b, "https://cdn.org/a123b/c", "", ResType::Other));
    assert!(!blocked(&b, "https://cdn.org/a123bc", "", ResType::Other), "^ must see a separator");
}

#[test]
fn matching_is_case_insensitive() {
    let b = bl("/AdFrame.\n||Ads.Example.COM^\n");
    assert!(blocked(&b, "https://X.org/ADFRAME.html", "", ResType::Subdocument));
    assert!(blocked(&b, "https://ADS.example.com/", "", ResType::Script));
}

#[test]
fn type_options_and_negations() {
    let b = bl("||a.com^$script\n||b.com^$image,stylesheet\n||c.com^$~script\n||d.com^$xmlhttprequest,ping\n||e.com^$subdocument\n");
    assert!(blocked(&b, "https://a.com/x", "", ResType::Script));
    assert!(!blocked(&b, "https://a.com/x", "", ResType::Image));
    assert!(blocked(&b, "https://b.com/x", "", ResType::Image) && blocked(&b, "https://b.com/x", "", ResType::Stylesheet) && !blocked(&b, "https://b.com/x", "", ResType::Script));
    assert!(!blocked(&b, "https://c.com/x", "", ResType::Script) && blocked(&b, "https://c.com/x", "", ResType::Image) && blocked(&b, "https://c.com/x", "", ResType::Xhr));
    assert!(blocked(&b, "https://d.com/x", "", ResType::Xhr) && blocked(&b, "https://d.com/x", "", ResType::Ping) && !blocked(&b, "https://d.com/x", "", ResType::Font));
    assert!(blocked(&b, "https://e.com/x", "", ResType::Subdocument) && !blocked(&b, "https://e.com/x", "", ResType::Script));
}

#[test]
fn a_rule_without_types_never_blocks_the_page_itself() {
    let b = bl("||news.org^\n");
    assert!(blocked(&b, "https://news.org/a.js", "https://other.org/", ResType::Script));
    assert!(!blocked(&b, "https://news.org/", "", ResType::Document), "the main page is only blocked by a rule that says $document");
    let d = bl("||bad.org^$document\n");
    assert!(blocked(&d, "https://bad.org/", "", ResType::Document));
    assert!(!blocked(&d, "https://bad.org/a.js", "https://x.org/", ResType::Script));
}

#[test]
fn third_party_and_first_party() {
    let b = bl("||metrics.com^$third-party\n||own.org/t.gif$~third-party\n");
    assert!(blocked(&b, "https://metrics.com/p", "https://news.org/", ResType::Image));
    assert!(!blocked(&b, "https://metrics.com/p", "https://metrics.com/", ResType::Image), "same site");
    assert!(!blocked(&b, "https://metrics.com/p", "https://www.metrics.com/a", ResType::Image), "same registrable domain");
    assert!(!blocked(&b, "https://metrics.com/p", "", ResType::Image), "unknown page: not third party, fails open");
    assert!(blocked(&b, "https://own.org/t.gif", "https://own.org/", ResType::Image));
    assert!(!blocked(&b, "https://own.org/t.gif", "https://x.org/", ResType::Image));
    assert_eq!(site_of("a.b.example.co.uk"), "example.co.uk");
    assert_eq!(site_of("www.example.com"), "example.com");
    assert_eq!(site_of("203.0.113.7"), "203.0.113.7");
    assert_eq!(site_of("localhost"), "localhost");
    assert_eq!(site_of("me.github.io"), "me.github.io");
}

#[test]
fn domain_option_includes_and_excludes() {
    let b = bl("/promo.js$domain=a.com|b.org\n/pop.js$domain=~safe.com\n/both.js$domain=x.com|~sub.x.com\n");
    assert!(blocked(&b, "https://cdn.net/promo.js", "https://a.com/p", ResType::Script));
    assert!(blocked(&b, "https://cdn.net/promo.js", "https://www.b.org/", ResType::Script));
    assert!(!blocked(&b, "https://cdn.net/promo.js", "https://c.com/", ResType::Script));
    assert!(!blocked(&b, "https://cdn.net/promo.js", "", ResType::Script), "no page known");
    assert!(blocked(&b, "https://cdn.net/pop.js", "https://other.com/", ResType::Script));
    assert!(!blocked(&b, "https://cdn.net/pop.js", "https://safe.com/", ResType::Script));
    assert!(!blocked(&b, "https://cdn.net/pop.js", "https://m.safe.com/", ResType::Script));
    assert!(blocked(&b, "https://cdn.net/both.js", "https://x.com/", ResType::Script));
    assert!(!blocked(&b, "https://cdn.net/both.js", "https://sub.x.com/", ResType::Script));
}

#[test]
fn exceptions_override_blocks_unless_important() {
    let b = bl("||ads.net^\n@@||ads.net/allowed/\n||strict.net^$important\n@@||strict.net^\n");
    assert!(blocked(&b, "https://ads.net/x.js", "https://a.org/", ResType::Script));
    assert!(!blocked(&b, "https://ads.net/allowed/x.js", "https://a.org/", ResType::Script));
    assert!(blocked(&b, "https://strict.net/x.js", "https://a.org/", ResType::Script), "$important beats an exception");
    // exceptions can carry options too
    let c = bl("||ads.net^\n@@||ads.net^$domain=partner.org\n");
    assert!(!blocked(&c, "https://ads.net/x.js", "https://partner.org/", ResType::Script));
    assert!(blocked(&c, "https://ads.net/x.js", "https://other.org/", ResType::Script));
}

#[test]
fn a_document_exception_switches_filtering_off_for_that_page() {
    let b = bl("||ads.net^\n@@||friendly.org^$document\n");
    assert!(blocked(&b, "https://ads.net/x.js", "https://news.org/", ResType::Script));
    assert!(!blocked(&b, "https://ads.net/x.js", "https://friendly.org/", ResType::Script));
    assert!(!blocked(&b, "https://ads.net/x.js", "https://www.friendly.org/a", ResType::Script));
    assert_eq!(b.stats().page_allows, 1);
}

#[test]
fn comments_headers_and_blank_lines_are_not_rules() {
    for l in ["", "   ", "! comment", "[Adblock Plus 2.0]", "!#include other.txt", "! Title: EasyList"] {
        assert_eq!(parse_line(l), Line::Nothing, "{l:?}");
    }
}

#[test]
fn unsupported_rules_are_skipped_and_counted_never_applied() {
    for (l, why) in [
        (r"/ads[0-9]+\.js/", "regular expression rule"),
        (r"/banner(1|2)/$script", "regular expression rule"),
        ("||x.com^$redirect=noop.js", "option not supported"),
        ("||x.com^$csp=script-src 'none'", "option not supported"),
        ("||x.com^$removeparam=utm", "option not supported"),
        ("||x.com^$badfilter", "option not supported"),
        ("||x.com^$popup", "option not supported"),
        ("example.com##+js(abort-on-property-read, x)", "selector not supported"),
        ("example.com#?#div:-abp-has(a)", "procedural or scriptlet rule"),
        ("##div:has-text(Ad)", "selector not supported"),
        ("*", "pattern with no text"),
        ("^^", "pattern with no text"),
        ("||", "pattern with no text"),
    ] {
        match parse_line(l) {
            Line::Skipped(w) => assert_eq!(w, why, "{l}"),
            other => panic!("{l} gave {other:?}"),
        }
    }
    let b = bl(r"/ads[0-9]+\.js/\n||x.com^$redirect=noop.js");
    assert_eq!(b.rule_count(), 0);
    assert!(b.stats().skipped >= 1);
}

#[test]
fn a_path_between_slashes_is_a_plain_rule_but_a_regular_expression_is_skipped() {
    match parse_line("/ads/") {
        Line::Net(r) => assert_eq!(r.pattern, "/ads/"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(parse_line("/ad(s|x)/"), Line::Skipped(_)));
    let b = bl("/ads/\n");
    assert!(blocked(&b, "https://x.org/ads/a.gif", "", ResType::Image));
}

#[test]
fn malformed_lines_never_panic() {
    let mut lines: Vec<String> = vec![
        "$".into(), "$$$$".into(), "@@".into(), "@@@@||".into(), "||$".into(), "|".into(), "||||x".into(), "|x|".into(), "x$domain=".into(), "x$domain=|".into(), "x$domain=~".into(), "x$,,,".into(), "x$=".into(), "x$~".into(), "##".into(), "#@#".into(), "a.com##".into(),
        "a.com##[".into(), "a.com##a[b".into(), "a.com##.".into(), "a.com##a >".into(), "a.com##> a".into(), "\u{0}\u{1}".into(), "||é.com^".into(), "x$script,image,~script,~image".into(), "x$~script,~image,~font,~media,~stylesheet,~xmlhttprequest,~subdocument,~ping,~websocket,~other".into(),
        "||".repeat(3000), "*".repeat(5000), "a".repeat(MAX_LINE + 1), "%%%%".into(), "\\\\\\".into(), "/".into(), "//".into(), "///".into(), "@@/$/".into(), "|||".into(),
    ];
    // every prefix of a realistic rule
    let rule = "@@||sub.example.com^*/ads/*.js|$script,third-party,domain=a.com|~b.org,important";
    for i in 0..rule.len() {
        if rule.is_char_boundary(i) {
            lines.push(rule[..i].to_string());
        }
    }
    let mut c = Compiler::new();
    for l in &lines {
        let _ = parse_line(l);
        c.add_list(l, 0);
    }
    let b = c.finish();
    for u in ["", "x", "://", "https://", "https:///", "https://a", "http://[::1]:80/", "https://u:p@h:1/x?y#z", "https://é.com/", "ftp://x"] {
        let _ = b.check(&ReqInfo { url: u, initiator: u, kind: ResType::Script });
    }
    // pseudo-random garbage
    let mut seed = 12345u64;
    for _ in 0..3000 {
        let mut s = String::new();
        for _ in 0..(seed % 40) {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let alphabet = b"|@$*^/.:-_~,=#abc123 \\()[]{}";
            s.push(alphabet[(seed >> 33) as usize % alphabet.len()] as char);
        }
        let _ = parse_line(&s);
    }
}

#[test]
fn matching_is_linear_even_for_adversarial_patterns() {
    // a regex engine would take exponential time on this; the glob matcher does a pass per piece
    let pat = format!("{}b", "*a".repeat(40));
    let b = bl(&format!("{pat}\n||x.org/{}\n", "a*".repeat(60)));
    let url = format!("https://x.org/{}", "a".repeat(1500));
    let t = std::time::Instant::now();
    for _ in 0..200 {
        let _ = b.check(&ReqInfo { url: &url, initiator: "", kind: ResType::Script });
    }
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn the_rule_count_is_bounded_and_dropped_rules_are_counted() {
    let mut text = String::new();
    for i in 0..500 {
        text.push_str(&format!("||site{i}.example^\n"));
    }
    let mut c = Compiler::with_limit(100);
    c.add_list(&text, 0);
    let b = c.finish();
    assert_eq!(b.rule_count(), 100);
    assert_eq!(b.stats().dropped, 400);
    assert!(blocked(&b, "https://site5.example/x", "", ResType::Script));
    assert!(!blocked(&b, "https://site400.example/x", "", ResType::Script), "past the limit: not kept");
    assert!(b.memory_estimate() > 0 && b.memory_estimate() < 1 << 20);
}

#[test]
fn rules_with_no_usable_word_are_capped() {
    let mut text = String::new();
    for i in 0..(MAX_FALLBACK + 50) {
        text.push_str(&format!("*{}*\n", "^".repeat(1 + i % 3) + "x"));
    }
    let b = bl(&text);
    assert!(b.stats().dropped >= 50);
}

fn synthetic_list(n: usize) -> String {
    let mut s = String::from("! synthetic\n");
    let mut seed = 99u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for i in 0..n {
        match i % 6 {
            0 | 1 => s.push_str(&format!("||ad{}.net{}.example^$third-party\n", next() % 1000, i)),
            2 => s.push_str(&format!("/track{}/pixel{}.gif\n", i, next() % 100)),
            3 => s.push_str(&format!("||cdn{}.org/banners/*/{}x{}^\n", i, next() % 900, next() % 900)),
            4 => s.push_str(&format!("@@||cdn{}.org/banners/ok^\n", i)),
            _ => s.push_str(&format!("-adserver-{}.$script,domain=site{}.com\n", i, i % 500)),
        }
    }
    s
}

#[test]
fn a_hundred_thousand_rules_compile_and_match_in_microseconds() {
    let text = synthetic_list(100_000);
    let t = std::time::Instant::now();
    let b = bl(&text);
    let compile = t.elapsed();
    assert!(b.rule_count() >= 99_000, "{}", b.rule_count());
    let reqs: Vec<String> = (0..2000).map(|i| format!("https://www.some-site-{}.com/static/js/app.{}.chunk.js?v={}", i, i * 7, i)).collect();
    let t = std::time::Instant::now();
    let mut hits = 0;
    for r in &reqs {
        if b.is_blocked(r, "https://www.news.org/", ResType::Script) {
            hits += 1;
        }
    }
    let per = t.elapsed().as_micros() as f64 / reqs.len() as f64;
    assert_eq!(hits, 0);
    // known hits
    let line = text.lines().find(|l| l.starts_with("||ad")).unwrap();
    let host = line.trim_start_matches("||").split('^').next().unwrap();
    assert!(b.is_blocked(&format!("https://{host}/x.js"), "https://www.news.org/", ResType::Script));
    assert!(!b.is_blocked(&format!("https://{host}/x.js"), "https://net0.example/", ResType::Script), "first party");
    let track = text.lines().find(|l| l.starts_with("/track")).unwrap();
    assert!(b.is_blocked(&format!("https://x.org{track}"), "", ResType::Image));
    eprintln!("100k rules: compile {compile:?}, {per:.1} us per request, about {} MB", b.memory_estimate() >> 20);
    // generous bounds so a loaded machine does not fail the test, tight enough to catch a scan over all rules
    assert!(per < 500.0, "{per} us per request");
    assert!(compile < std::time::Duration::from_secs(20), "{compile:?}");
    assert!(b.memory_estimate() < 120 << 20, "{}", b.memory_estimate());
}

#[test]
fn describe_shows_the_rule_as_a_list_would() {
    let b = bl("||ads.net^$third-party,script,domain=a.com|~b.com\n@@/ok/*$image\n");
    let v = b.check(&ReqInfo { url: "https://ads.net/x.js", initiator: "https://a.com/", kind: ResType::Script });
    let Verdict::Block { rule } = v else { panic!("{v:?}") };
    assert_eq!(b.describe(rule), "||ads.net^$script,third-party,domain=a.com|~b.com");
    assert_eq!(b.list_of(rule), Some(0));
}

#[test]
fn element_hiding_rules_generic_specific_and_exceptions() {
    let b = bl("##.ad-banner\nexample.com###sidebar-ad\nexample.com,news.org##div[data-ad=\"1\"] > a\n~example.com##.promo\nexample.com#@#.ad-banner\n##div:has-text(x)\n");
    let on_example = b.hide_selectors("www.example.com");
    assert!(on_example.contains(&"#sidebar-ad".to_string()) && on_example.contains(&"div[data-ad=\"1\"] > a".to_string()));
    assert!(!on_example.contains(&".ad-banner".to_string()), "excepted on example.com");
    assert!(!on_example.contains(&".promo".to_string()));
    let elsewhere = b.hide_selectors("other.org");
    assert!(elsewhere.contains(&".ad-banner".to_string()) && elsewhere.contains(&".promo".to_string()) && !elsewhere.contains(&"#sidebar-ad".to_string()));
    assert_eq!(b.stats().skipped, 1, "the :has-text rule");
}

#[test]
fn only_safe_selectors_are_accepted() {
    for s in ["div", ".a", "#b", "a.b#c", "div > a", "ul li + li", "a[href]", "a[href=\"x\"]", "a[href^=\"https://ads.\"]", "*.ad", "div.ad-box_1 ~ p"] {
        assert!(safe_selector(s), "{s}");
    }
    for s in ["", "div:hover", "a,b", "a >", "> a", "a{}", "a[href=\"x]", ".", "#", "a[", "div::before", "a\\b", "a[href=\"x\"][", "a[=x]", "a[b=\"\"]", "<script>", "a[b=x;y]"] {
        assert!(!safe_selector(s), "{s}");
    }
}

#[test]
fn host_and_site_helpers() {
    assert_eq!(host_of("https://User:pw@WWW.Example.com:8443/a?b#c"), "www.example.com");
    assert_eq!(host_of("www.example.com"), "www.example.com");
    assert_eq!(host_of("https://[::1]:80/x"), "[::1");
    assert_eq!(host_of("https://example.com./"), "example.com");
    assert!(under("a.b.com", "b.com") && under("b.com", "b.com") && !under("ab.com", "b.com") && !under("b.com", "a.b.com"));
}

// ---------- WebKit rule list ----------

fn export(text: &str) -> (blocker_json::JsonList, Blocker) {
    let b = bl(text);
    (blocker_json::export(&b, blocker_json::DEFAULT_LIMIT, true), b)
}

#[test]
fn the_webkit_list_is_valid_json_with_the_expected_shapes() {
    let (j, _) = export("||ads.example.com^\n|https://t.net/p*.gif|\n/banner/*/img^$image,third-party\n@@||ads.example.com/ok/\n||x.com^$important\n||s.com^$script,domain=a.com|b.org\n##.ad\nexample.com##.top\n");
    assert!(blocker_json::looks_valid(&j.json));
    let v = mini_json(&j.json);
    let arr = v.as_array();
    assert_eq!(arr.len(), j.rules);
    let text = &j.json;
    assert!(text.contains(r#""url-filter":"^[^:]+://+([^:/]+\\.)?ads\\.example\\.com[^a-z0-9_.%-]""#), "{text}");
    assert!(text.contains(r#""url-filter":"^https://t\\.net/p.*\\.gif$""#), "{text}");
    assert!(text.contains(r#""load-type":["third-party"]"#) && text.contains(r#""resource-type":["image","svg-document"]"#));
    assert!(text.contains(r#""if-domain":["*a.com","*b.org"]"#));
    assert!(text.contains(r#""type":"ignore-previous-rules""#) && text.contains(r#""type":"css-display-none""#));
    // order: normal blocks, exception, important block, hiding
    let p_block = text.find("ads\\\\.example").unwrap();
    let p_exc = text.find("ignore-previous-rules").unwrap();
    let p_imp = text.find("x\\\\.com").unwrap();
    let p_hide = text.find("css-display-none").unwrap();
    assert!(p_block < p_exc && p_exc < p_imp && p_imp < p_hide);
    assert!(text.contains(r#""selector":".ad""#) && text.contains(r#""if-domain":["*example.com"]"#));
}

#[test]
fn the_webkit_list_never_blocks_pages_without_asking() {
    let (j, _) = export("||ads.net^\n||frames.net^$subdocument\n||both.net^$script,subdocument\n||page.net^$document\n");
    // a rule with no types must not name "document"
    let first = j.json.split("},{").find(|r| r.contains("ads\\\\.net")).unwrap();
    assert!(!first.contains("\"document\""), "{first}");
    let frame = j.json.split("},{").find(|r| r.contains("frames\\\\.net")).unwrap();
    assert!(frame.contains("\"load-context\":[\"child-frame\"]") && frame.contains("\"document\""));
    // script + subdocument gives two rules
    assert_eq!(j.json.matches("both\\\\.net").count(), 2);
    assert!(j.json.contains("page\\\\.net"));
}

#[test]
fn rules_webkit_cannot_say_are_dropped_and_counted() {
    let (j, _) = export("||a.com^$domain=x.org|~y.org\n||b.com^\n");
    assert_eq!(j.unsupported, 1);
    assert!(j.json.contains("b\\\\.com") && !j.json.contains("a\\\\.com"));
    let (k, _) = export("||bücher.de^\n");
    assert_eq!(k.rules, 0);
}

#[test]
fn the_webkit_list_respects_the_rule_limit_and_keeps_exceptions() {
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&format!("||s{i}.example^\n"));
    }
    text.push_str("@@||s1.example/ok/\n||imp.example^$important\n");
    let b = bl(&text);
    let j = blocker_json::export(&b, 100, false);
    assert!(j.rules <= 100, "{}", j.rules);
    assert!(j.over_limit >= 300);
    assert!(j.json.contains("ignore-previous-rules") && j.json.contains("imp\\\\.example"));
    // the hard limit of the engine
    let j = blocker_json::export(&b, 10_000_000, false);
    assert!(j.rules <= blocker_json::WEBKIT_MAX_RULES);
}

#[test]
fn allowed_sites_get_a_last_rule_that_cancels_the_earlier_ones() {
    let b = bl("||ads.net^\n##.ad\n");
    let j = blocker_json::export_allowing(&b, 1000, true, &["news.org".to_string(), "bad host".to_string(), "".to_string()]);
    let _ = mini_json(&j.json);
    let last = j.json.rfind("{\"trigger\"").unwrap();
    assert!(j.json[last..].contains("\"if-domain\":[\"*news.org\"]") && j.json[last..].contains("ignore-previous-rules"));
    assert_eq!(j.json.matches("*news.org").count(), 1, "names with spaces are not written");
}

#[test]
fn when_the_limit_bites_whole_domain_rules_are_kept_first() {
    let mut text = String::new();
    for i in 0..300 {
        text.push_str(&format!("/path{i}/x.gif\n"));
    }
    for i in 0..50 {
        text.push_str(&format!("||dom{i}.example^\n"));
    }
    let b = bl(&text);
    let j = blocker_json::export(&b, 60, false);
    assert!(j.rules <= 60);
    assert_eq!(j.json.matches("dom").count(), 50, "every domain rule is in");
}

#[test]
fn the_webkit_list_parses_as_json_for_awkward_rules() {
    let (j, _) = export("||a.com^\n/p\"q\\r/x\n*quote\"\n|http://x.org/a b\n||c.com/\\\\x\n");
    let _ = mini_json(&j.json);
}

/// A strict little JSON reader, only for these tests: checks the structure the exporter writes.
#[derive(Debug)]
enum J {
    Arr(Vec<J>),
    #[allow(dead_code)]
    Obj(Vec<(String, J)>),
    Str(String),
}

impl J {
    fn as_array(&self) -> &Vec<J> {
        match self {
            J::Arr(a) => a,
            o => panic!("{o:?}"),
        }
    }
}

fn mini_json(s: &str) -> J {
    fn ws(b: &[u8], i: &mut usize) {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn val(b: &[u8], i: &mut usize) -> J {
        ws(b, i);
        match b[*i] {
            b'[' => {
                *i += 1;
                let mut v = vec![];
                ws(b, i);
                if b[*i] == b']' {
                    *i += 1;
                    return J::Arr(v);
                }
                loop {
                    v.push(val(b, i));
                    ws(b, i);
                    match b[*i] {
                        b',' => *i += 1,
                        b']' => {
                            *i += 1;
                            return J::Arr(v);
                        }
                        c => panic!("bad {} at {}", c as char, *i),
                    }
                }
            }
            b'{' => {
                *i += 1;
                let mut v = vec![];
                loop {
                    ws(b, i);
                    let J::Str(k) = val(b, i) else { panic!("key") };
                    ws(b, i);
                    assert_eq!(b[*i], b':');
                    *i += 1;
                    v.push((k, val(b, i)));
                    ws(b, i);
                    match b[*i] {
                        b',' => *i += 1,
                        b'}' => {
                            *i += 1;
                            return J::Obj(v);
                        }
                        c => panic!("bad {} at {}", c as char, *i),
                    }
                }
            }
            b'"' => {
                *i += 1;
                let mut out = String::new();
                loop {
                    match b[*i] {
                        b'"' => {
                            *i += 1;
                            return J::Str(out);
                        }
                        b'\\' => {
                            match b[*i + 1] {
                                b'n' => out.push('\n'),
                                b'r' => out.push('\r'),
                                b't' => out.push('\t'),
                                b'u' => {
                                    out.push('?');
                                    *i += 4;
                                }
                                c => out.push(c as char),
                            }
                            *i += 2;
                        }
                        c => {
                            assert!(c >= 0x20, "raw control character");
                            out.push(c as char);
                            *i += 1;
                        }
                    }
                }
            }
            c => panic!("unexpected {} at {}", c as char, *i),
        }
    }
    let b = s.as_bytes();
    let mut i = 0;
    let v = val(b, &mut i);
    ws(b, &mut i);
    assert_eq!(i, b.len(), "trailing text");
    v
}

// ---------- lists ----------

#[test]
fn sha256_matches_known_values() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(sha256_hex(&vec![b'a'; 1000]), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
}

#[derive(Default)]
struct FakeServer {
    pages: Mutex<HashMap<String, Result<Vec<u8>, String>>>,
    asked: Mutex<Vec<String>>,
}

impl ListSource for FakeServer {
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        self.asked.lock().unwrap().push(url.to_string());
        match self.pages.lock().unwrap().get(url) {
            Some(Ok(b)) if b.len() > max => Err("the list is larger than the limit".into()),
            Some(r) => r.clone(),
            None => Err("the server answered 404".into()),
        }
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-blk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn a_known_list_is_downloaded_stored_with_hash_licence_and_time_and_compiled() {
    let dir = tmp("known");
    let store = ListStore::open(&dir);
    let srv = FakeServer::default();
    let k = crate::blocker_lists::KNOWN[0];
    assert!(k.licence.contains("GPL") && k.url.starts_with("https://"));
    srv.pages.lock().unwrap().insert(k.url.into(), Ok(b"! Title: x\n||ads.example.com^\n##.ad\n".to_vec()));
    assert!(store.list().is_empty(), "nothing is downloaded until asked");
    assert!(!store.compile().is_blocked("https://ads.example.com/x", "https://a.org/", ResType::Script));
    let m = store.add_known(&k, &srv, 1000).unwrap();
    assert_eq!(m.id, "easylist");
    assert_eq!(m.hash, sha256_hex(b"! Title: x\n||ads.example.com^\n##.ad\n"));
    assert_eq!((m.updated, m.rules, m.enabled), (1000, 2, true));
    assert_eq!(store.list(), vec![m.clone()]);
    assert!(store.compile().is_blocked("https://ads.example.com/x", "https://a.org/", ResType::Script));
    // switched off: fails open
    assert!(store.set_enabled("easylist", false));
    assert!(!store.compile().is_blocked("https://ads.example.com/x", "https://a.org/", ResType::Script));
    assert!(store.set_enabled("easylist", true));
    // refresh: unchanged text keeps the hash and renews the time; new text changes the hash
    let r = store.refresh("easylist", &srv, 2000).unwrap();
    assert_eq!((r.hash.clone(), r.updated), (m.hash.clone(), 2000));
    srv.pages.lock().unwrap().insert(k.url.into(), Ok(b"||other.example^\n".to_vec()));
    let r2 = store.refresh("easylist", &srv, 3000).unwrap();
    assert_ne!(r2.hash, m.hash);
    assert!(store.compile().is_blocked("https://other.example/x", "https://a.org/", ResType::Script));
    assert!(store.remove("easylist") && store.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_download_or_a_page_that_is_not_a_list_keeps_the_old_list() {
    let dir = tmp("fail");
    let store = ListStore::open(&dir);
    let srv = FakeServer::default();
    srv.pages.lock().unwrap().insert("https://l.example/a.txt".into(), Ok(b"||ads.example.com^\n".to_vec()));
    let m = store.add_url("https://l.example/a.txt", &srv, 10).unwrap();
    assert!(m.id.starts_with("custom-") && m.licence.contains("unknown") && m.name == "l.example");
    srv.pages.lock().unwrap().insert("https://l.example/a.txt".into(), Err("connection refused".into()));
    assert!(store.refresh(&m.id, &srv, 20).is_err());
    assert!(store.compile().is_blocked("https://ads.example.com/x", "https://a.org/", ResType::Script), "old text kept");
    srv.pages.lock().unwrap().insert("https://l.example/a.txt".into(), Ok(b"<html><body>Not found</body></html>".to_vec()));
    let e = store.refresh(&m.id, &srv, 30).unwrap_err();
    assert!(e.contains("does not look like a filter list"), "{e}");
    assert_eq!(store.list()[0].updated, 10);
    assert!(store.add_url("not a url", &srv, 1).is_err());
    assert!(store.add_url("https://l.example/missing.txt", &srv, 1).is_err());
    // the same address again replaces, not duplicates
    srv.pages.lock().unwrap().insert("https://l.example/a.txt".into(), Ok(b"||new.example^\n".to_vec()));
    store.add_url("https://l.example/a.txt", &srv, 40).unwrap();
    assert_eq!(store.list().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_huge_download_is_refused() {
    let dir = tmp("huge");
    let store = ListStore::open(&dir);
    let big = vec![b'a'; crate::blocker_lists::MAX_LIST_BYTES + 1];
    assert!(store.put("big", "Big", "", "", &big, 1).unwrap_err().contains("24 MB"));
    assert!(store.put("../evil", "x", "", "", b"||a.com^\n", 1).is_err());
    assert!(store.put("a b", "x", "", "", b"||a.com^\n", 1).is_err());
    assert!(store.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_weekly_refresh_only_touches_old_lists_with_an_address() {
    let dir = tmp("weekly");
    let store = ListStore::open(&dir);
    let srv = FakeServer::default();
    for u in ["https://a.example/l.txt", "https://b.example/l.txt"] {
        srv.pages.lock().unwrap().insert(u.into(), Ok(b"||x.example^\n".to_vec()));
    }
    let a = store.add_url("https://a.example/l.txt", &srv, 1_000).unwrap();
    store.add_url("https://b.example/l.txt", &srv, 1_000_000).unwrap();
    let f = dir.join("mine.txt");
    std::fs::write(&f, "||mine.example^\n").unwrap();
    store.add_file(&f, 5).unwrap();
    srv.asked.lock().unwrap().clear();
    let done = store.refresh_due(&srv, 1_000_000 + 100);
    assert_eq!(done.len(), 1, "a is a week old; b is new; the file has no address");
    assert_eq!(done[0].0, a.id);
    assert_eq!(*srv.asked.lock().unwrap(), vec!["https://a.example/l.txt".to_string()]);
    assert!(store.compile().is_blocked("https://mine.example/x", "https://a.org/", ResType::Script));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_meta_file_gives_no_lists_and_no_blocking() {
    let dir = tmp("dmg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lists.meta"), "garbage\n\u{0}\t\t\n../x\ta\tb\tc\td\t1\t2\t3\t1\n").unwrap();
    let store = ListStore::open(&dir);
    assert!(store.list().is_empty());
    assert_eq!(store.compile().rule_count(), 0);
    // a listed file that disappeared is left out
    let srv = FakeServer::default();
    srv.pages.lock().unwrap().insert("https://l.example/a.txt".into(), Ok(b"||ads.example.com^\n".to_vec()));
    let m: ListMeta = store.add_url("https://l.example/a.txt", &srv, 1).unwrap();
    std::fs::remove_file(dir.join(format!("{}.txt", m.id))).unwrap();
    assert_eq!(store.compile().rule_count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- fetch layer ----------

#[derive(Default)]
struct FakeNet {
    seen: Mutex<Vec<String>>,
}

impl Fetch for FakeNet {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.seen.lock().unwrap().push(url.to_string());
        Ok(Response { status: 200, content_type: "text/html".into(), body: b"<html>ok</html>".to_vec(), ..Response::default() })
    }
}

fn setup(rules: &str) -> (Arc<FakeNet>, Arc<Filter>, FilteredFetch) {
    let net = Arc::new(FakeNet::default());
    let f = Filter::new();
    f.set_blocker(bl(rules));
    let ff = FilteredFetch::new(net.clone(), f.clone());
    (net, f, ff)
}

fn page_req(url: &str) -> Request {
    Request { priority: crate::net::DOCUMENT, ..Request::get(&Url::parse(url).unwrap()) }
}

fn sub_req(url: &str, referer: &str) -> Request {
    Request { referer: referer.into(), priority: crate::net::BLOCKING, ..Request::get(&Url::parse(url).unwrap()) }
}

#[test]
fn a_blocked_request_is_never_sent_and_is_counted_and_listed() {
    let (net, f, ff) = setup("||ads.example.com^\n||track.net^$third-party\n");
    assert!(ff.send(&page_req("https://news.org/")).is_ok());
    assert!(ff.send(&sub_req("https://news.org/app.js", "https://news.org/")).is_ok());
    let e = ff.send(&sub_req("https://ads.example.com/a.js", "https://news.org/")).unwrap_err();
    assert!(e.contains("blocked by your filter list") && e.contains("||ads.example.com^"), "{e}");
    assert!(ff.send(&sub_req("https://track.net/p.gif", "https://news.org/x")).is_err());
    assert_eq!(*net.seen.lock().unwrap(), vec!["https://news.org/".to_string(), "https://news.org/app.js".to_string()]);
    assert_eq!(f.page_blocked(), ("news.org".to_string(), 2));
    let log = f.page_log();
    assert_eq!(log.len(), 2);
    assert_eq!((log[0].url.as_str(), log[0].kind, log[0].rule.as_str()), ("https://ads.example.com/a.js", ResType::Script, "||ads.example.com^"));
    assert_eq!(log[1].kind, ResType::Image);
    assert_eq!(f.totals().0, 2);
    // the next page starts at zero
    assert!(ff.send(&page_req("https://other.org/")).is_ok());
    assert_eq!(f.page_blocked(), ("other.org".to_string(), 0));
    assert!(f.page_log().is_empty());
}

#[test]
fn without_a_referrer_the_page_now_shown_is_the_initiator() {
    let (_, _, ff) = setup("||track.net^$third-party\n");
    ff.send(&page_req("https://news.org/")).unwrap();
    assert!(ff.send(&sub_req("https://track.net/p.gif", "")).is_err());
    ff.send(&page_req("https://track.net/")).unwrap();
    assert!(ff.send(&sub_req("https://track.net/p.gif", "")).is_ok(), "now the same site");
}

#[test]
fn the_switch_and_the_per_site_allow_list_turn_blocking_off() {
    let (net, f, ff) = setup("||ads.example.com^\n");
    ff.send(&page_req("https://news.org/")).unwrap();
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://news.org/")).is_err());
    let mut allow = BTreeSet::new();
    allow.insert("news.org".to_string());
    f.configure(true, &allow);
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://news.org/")).is_ok(), "allowed on this site");
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://www.news.org/p")).is_ok(), "and its subdomains");
    ff.send(&page_req("https://other.org/")).unwrap();
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://other.org/")).is_err(), "other sites still filtered");
    f.configure(false, &BTreeSet::new());
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://other.org/")).is_ok(), "switch off");
    assert!(net.seen.lock().unwrap().iter().any(|u| u.contains("ads.example.com")));
}

#[test]
fn a_blocked_page_gets_an_explanation_never_a_silent_failure() {
    let (net, f, ff) = setup("||bad.org^$document\n");
    let r = ff.send(&page_req("https://bad.org/path")).unwrap();
    let html = String::from_utf8(r.body).unwrap();
    assert!(html.contains(INTERSTITIAL_TITLE) && html.contains("bad.org") && html.contains("||bad.org^$document") && html.contains("Allow on this site"));
    assert!(net.seen.lock().unwrap().is_empty());
    assert_eq!(f.page_blocked().1, 1);
    // one click: allow, and the page opens
    let mut allow = BTreeSet::new();
    allow.insert("bad.org".to_string());
    f.configure(true, &allow);
    let r = ff.send(&page_req("https://bad.org/path")).unwrap();
    assert!(String::from_utf8(r.body).unwrap().contains("ok"));
    assert_eq!(net.seen.lock().unwrap().len(), 1);
}

#[test]
fn the_explanation_page_escapes_what_it_shows() {
    let b = crate::blocker_state::Blocked { page: "x".into(), url: "https://a.org/<script>alert(1)</script>".into(), kind: ResType::Document, rule: "||a.org/<b>\"&".into() };
    let h = interstitial_html(&b);
    assert!(!h.contains("<script>") && !h.contains("<b>") && h.contains("&lt;b&gt;&quot;&amp;"));
}

#[test]
fn no_lists_means_everything_passes() {
    let net = Arc::new(FakeNet::default());
    let f = Filter::new();
    let ff = FilteredFetch::new(net.clone(), f.clone());
    assert!(ff.send(&sub_req("https://ads.example.com/a.js", "https://news.org/")).is_ok());
    assert_eq!(f.totals(), (0, 0));
}

#[test]
fn the_wrapper_passes_per_user_calls_through() {
    struct PerUser;
    impl Fetch for PerUser {
        fn get(&self, _: &Url) -> Result<Response, String> {
            Ok(Response { status: 200, ..Response::default() })
        }
        fn for_user(&self, _: &str) -> Option<Arc<dyn Fetch>> {
            Some(Arc::new(PerUser))
        }
    }
    let f = Filter::new();
    f.set_blocker(bl("||ads.example.com^\n"));
    let ff = FilteredFetch::new(Arc::new(PerUser), f);
    let u = ff.for_user("amy").unwrap();
    assert!(u.send(&sub_req("https://ads.example.com/a.js", "https://news.org/")).is_err(), "the per-user transport is filtered too");
    assert!(FilteredFetch::new(Arc::new(FakeNet::default()), Filter::new()).for_user("amy").is_none());
}
