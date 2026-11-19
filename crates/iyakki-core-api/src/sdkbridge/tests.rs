use super::markdown::analyze;
use super::money::*;
use super::*;

fn all(_: &str) -> bool {
    true
}
fn none(_: &str) -> bool {
    false
}

#[test]
fn the_manifest_is_consistent_names_are_unique_and_every_since_is_a_version_not_newer_than_this_release() {
    let mut seen = std::collections::BTreeSet::new();
    for n in NAMESPACES {
        assert!(seen.insert(n.name), "namespace {} twice", n.name);
        let v = Version::parse(n.since).unwrap_or_else(|| panic!("{} since", n.name));
        assert!(v <= Version::CURRENT, "{} is newer than this release", n.name);
        if let Some(c) = n.capability {
            assert!(capability(c).is_some(), "{}: capability {c} is not listed", n.name);
        }
        let mut ops = std::collections::BTreeSet::new();
        for o in n.ops {
            assert!(ops.insert(o.name), "{}.{} twice", n.name, o.name);
            assert!(Version::parse(o.since).unwrap() >= v, "{}.{} is older than its namespace", n.name, o.name);
            assert!(Version::parse(o.since).unwrap() <= Version::CURRENT);
            if let Some(c) = o.capability {
                assert!(capability(c).is_some(), "{}.{}: capability {c}", n.name, o.name);
            }
        }
    }
    let mut keys = std::collections::BTreeSet::new();
    for c in CAPABILITIES {
        assert!(keys.insert(c.key), "capability {} twice", c.key);
        assert!(Version::parse(c.since).unwrap() <= Version::CURRENT);
        assert!(!c.words.is_empty() && c.words.len() < 400, "{} words", c.key);
        assert!(!c.key.contains('=') && !c.key.contains(','), "{}: the permission string uses = and ,", c.key);
    }
    assert_eq!(SDK_VERSION, Version::CURRENT.text());
}

#[test]
fn a_call_that_changes_spends_or_listens_always_asks_and_money_and_the_microphone_never_run_free() {
    for n in NAMESPACES {
        for o in n.ops {
            let full = format!("{}.{}", n.name, o.name);
            match o.effect {
                Effect::Read | Effect::Own => {}
                Effect::Change => assert_ne!(o.approval, Approval::None, "{full} changes something and must ask"),
                Effect::Spend => assert_eq!(o.approval, Approval::Limit, "{full}"),
                Effect::Listen => assert_eq!(o.approval, Approval::EachCall, "{full}"),
            }
        }
    }
    let (n, o) = find_op("speech.listen").unwrap();
    assert_eq!(capability_of(n, o), Some("microphone"), "listening is its own capability, not the voice's");
    assert!(capability("money").unwrap().sensitive && capability("microphone").unwrap().sensitive && capability("k3s").unwrap().sensitive);
    // the calls the spec says only read stay free of cards
    for full in ["markdown.render", "dash.sources", "flows.list", "flows.status", "k3s.status", "k3s.get", "k3s.logs", "money.limits", "media.state", "audio.outputs"] {
        let (_, o) = find_op(full).unwrap();
        assert_eq!(o.effect, Effect::Read, "{full}");
        assert!(matches!(o.approval, Approval::None | Approval::FirstUse), "{full}");
    }
}

#[test]
fn names_resolve_with_dots_in_the_call_and_unknown_names_do_not() {
    assert!(find_op("dash.panel.add").is_some());
    assert!(find_op("media.queue.add").is_some());
    assert!(find_op("markdown.future").is_none());
    assert!(find_op("hologram.show").is_none());
    assert!(find_op("nodots").is_none());
    assert!(find_op("").is_none());
}

#[test]
fn discovery_lists_what_the_host_knows_and_whether_the_app_may_use_it() {
    let l = listing(Version::CURRENT, &|k: &str| k == "markdown");
    assert_eq!(l["sdk"], SDK_VERSION);
    let ns = l["namespaces"].as_array().unwrap();
    let md = ns.iter().find(|n| n["name"] == "markdown").unwrap();
    assert_eq!(md["ready"], true);
    assert_eq!(md["since"], "1.2");
    assert_eq!(md["ops"][0]["name"], "render");
    let money = ns.iter().find(|n| n["name"] == "money").unwrap();
    assert_eq!(money["ready"], false, "known but not declared by this app");
    // the speech namespace is ready for say only when `speech` is held, and listen only with `microphone`
    let sp = |held: &'static str| listing(Version::CURRENT, &move |k: &str| k == held)["namespaces"].as_array().unwrap().iter().find(|n| n["name"] == "speech").cloned().unwrap();
    let ready = |n: &Value, op: &str| n["ops"].as_array().unwrap().iter().find(|o| o["name"] == op).unwrap()["ready"].as_bool().unwrap();
    assert!(ready(&sp("speech"), "say") && !ready(&sp("speech"), "listen"));
    assert!(!ready(&sp("microphone"), "say") && ready(&sp("microphone"), "listen"));
}

#[test]
fn has_answers_for_a_namespace_a_call_and_a_name_nobody_knows() {
    let v = Version::CURRENT;
    assert!(has(v, "markdown", &all) && has(v, "markdown.render", &all) && has(v, "dash.panel.add", &all));
    assert!(!has(v, "markdown", &none), "known, but the app did not declare it");
    assert!(has(v, "sdk", &none) && has(v, "sdk.has", &none), "discovery needs nothing");
    assert!(!has(v, "hologram", &all) && !has(v, "markdown.future", &all) && !has(v, "", &all));
}

#[test]
fn an_older_host_does_not_know_the_newer_calls_and_says_so_in_a_structured_way() {
    let old = Version { major: 1, minor: 1 };
    assert!(!has(old, "markdown", &all) && !has(old, "speech.listen", &all) && !has(old, "money.spend", &all));
    assert!(has(old, "speech.say", &all) && has(old, "llm.chat", &all) && has(old, "storage.get", &all), "what the older host had keeps working");
    let l = listing(old, &all);
    assert_eq!(l["sdk"], "1.1");
    assert!(l["namespaces"].as_array().unwrap().iter().all(|n| n["name"] != "markdown" && n["name"] != "money"));
    let sp = l["namespaces"].as_array().unwrap().iter().find(|n| n["name"] == "speech").unwrap();
    assert!(sp["ops"].as_array().unwrap().iter().all(|o| o["name"] != "listen"), "a newer call of an older namespace is left out too");
    assert!(l["capabilities"].as_array().unwrap().iter().all(|c| c["key"] != "money"));
    let e = unsupported("markdown.render", old);
    assert_eq!(e["code"], "Unsupported");
    assert_eq!(e["sdk"], "1.1");
    assert_eq!(e["namespace"], "markdown");
    assert!(e["message"].as_str().unwrap().contains("not available in this version"));
}

#[test]
fn the_unsupported_answer_is_bounded_and_clean_for_any_name() {
    let e = unsupported(&format!("evil\u{0}\n{}.x", "a".repeat(500)), Version::CURRENT);
    assert!(e["op"].as_str().unwrap().chars().count() <= 60 && !e["message"].as_str().unwrap().contains('\n'));
    assert_eq!(unsupported("x", Version::CURRENT)["namespace"], "x");
}

#[test]
fn an_app_states_the_sdk_it_needs_and_a_newer_need_is_refused_in_words() {
    let v = Version::CURRENT;
    assert!(needs_ok(v, "1.0").is_ok() && needs_ok(v, "1.2").is_ok());
    assert!(needs_ok(v, "1.3").unwrap_err().contains("update IYAKKI"));
    assert!(needs_ok(v, "2.0").unwrap_err().contains("written for SDK 2.0"));
    assert!(needs_ok(v, "soon").is_err() && needs_ok(v, "1").is_err() && needs_ok(v, "1.x").is_err());
    assert_eq!(Version::parse("1.10"), Some(Version { major: 1, minor: 10 }));
    assert!(Version { major: 1, minor: 10 } > Version { major: 1, minor: 9 }, "versions compare as numbers, not as text");
}

#[test]
fn grants_are_checked_normalised_and_survive_the_permission_string() {
    let ok = |k: &str, v: Value| normalize_grant(k, &v).unwrap();
    assert_eq!(ok("markdown", json!(true)), Some(json!(true)));
    assert_eq!(ok("markdown", json!(false)), None);
    assert_eq!(ok("flows", json!(["Backup", "report", "Backup"])), Some(json!(["Backup", "report"])), "duplicates collapse");
    assert_eq!(ok("flows", json!([])), None);
    assert_eq!(ok("embed_hosts", json!(["Docs.Example.com"])), Some(json!(["docs.example.com"])));
    assert_eq!(ok("k3s", json!("read")), Some(json!("read")));
    assert_eq!(ok("money", json!("EUR")), Some(json!("EUR")));
    for (k, v) in [
        ("markdown", json!("yes")),
        ("flows", json!("backup")),
        ("flows", json!([1])),
        ("flows", json!(["a,b"])),
        ("flows", json!(["a=b"])),
        ("flows", json!((0..9).map(|i| format!("f{i}")).collect::<Vec<_>>())),
        ("embed", json!(["flash"])),
        ("embed_hosts", json!(["*"])),
        ("embed_hosts", json!(["https://x.example.com/p"])),
        ("k3s", json!("root")),
        ("k3s", json!(true)),
        ("money", json!("usd")),
        ("money", json!("DOLLARS")),
        ("money", json!(5)),
        ("hologram", json!(true)),
    ] {
        assert!(normalize_grant(k, &v).is_err(), "{k} {v}");
    }
    for (k, v) in [("markdown", json!(true)), ("flows", json!(["a b", "c.d"])), ("k3s", json!("manage")), ("money", json!("JPY")), ("embed", json!(["markdown", "browser"])), ("audio", json!(true))] {
        let p = permission_string(k, &v);
        let (key, back) = parse_permission(p.strip_prefix("tile:").unwrap()).unwrap();
        assert_eq!((key.as_str(), back), (k, v), "{p}");
    }
    assert!(parse_permission("hologram").is_none() && parse_permission("storage").is_none(), "unknown and original keys are not v2 grants");
    assert!(parse_permission("flows").is_none() && parse_permission("markdown=1").is_none());
    assert!(grant_words("flows", &json!(["a", "b"])).unwrap().contains("[a, b]"));
    assert!(grant_words("money", &json!("USD")).unwrap().contains("before every payment"));
}

#[test]
fn markdown_is_analysed_without_marks_with_links_collected_and_dangerous_links_counted_not_listed() {
    let md = "# Title\n\nSome **bold** and _italic_ and `code` text with a [link](https://example.com/a) and snake_case.\n\n- one\n- two\n1. first\n> quoted\n\n```\nlet x = 1;\n```\n![alt text](pic.png) [bad](javascript:alert(1)) [data](data:text/html;base64,AA)\n---\n";
    let a = analyze(md, 640).unwrap();
    assert!(a.plain.starts_with("Title"), "{}", a.plain);
    assert!(a.plain.contains("Some bold and italic and code text with a link and snake_case."), "{}", a.plain);
    assert!(a.plain.contains("let x = 1;") && !a.plain.contains("```"));
    assert!(a.plain.contains("one\ntwo\nfirst\nquoted"), "{}", a.plain);
    assert_eq!(a.links, vec![("link".to_string(), "https://example.com/a".to_string())]);
    assert_eq!(a.blocked_links, 2);
    assert_eq!(a.images, 1);
    assert!(a.plain.contains("alt text"));
    assert!(a.lines >= 10);
    let j = a.to_json();
    assert_eq!(j["links"][0]["url"], "https://example.com/a");
    assert_eq!(j["blockedLinks"], 2);
}

#[test]
fn the_line_count_follows_the_width_and_the_limits_hold() {
    let long = "word ".repeat(100); // 500 characters
    let wide = analyze(&long, 800).unwrap().lines; // 100 per line
    let narrow = analyze(&long, 400).unwrap().lines; // 50 per line
    assert_eq!((wide, narrow), (5, 10));
    assert_eq!(analyze("", 0).unwrap().lines, 0);
    assert_eq!(analyze("a\n\nb", 0).unwrap().lines, 3);
    assert!(analyze(&"x".repeat(MAX_TEXT_BYTES + 1), 0).is_err());
    let many: String = (0..800).map(|i| format!("[l{i}](https://e.example/{i}) ")).collect();
    assert_eq!(analyze(&many, 0).unwrap().links.len(), super::markdown::MAX_LINKS);
    // text that is not Markdown at all is just text; unbalanced marks do not hang or panic
    for odd in ["[[[", "](", "![", "`", "**", "[a](", "[a](b", "\\", "\u{0}", "[a]\n(b)"] {
        assert!(analyze(odd, 0).is_ok(), "{odd:?}");
    }
}

const MAX_TEXT_BYTES: usize = super::markdown::MAX_TEXT;

#[test]
fn money_defaults_to_nothing_and_the_limit_is_checked_before_every_spend() {
    assert_eq!(check_spend(None, 0, "USD", 100), Err(Refusal::NoLimit), "the default is zero");
    let zero = Limit { currency: "USD".into(), per_day_minor: 0 };
    assert_eq!(check_spend(Some(&zero), 0, "USD", 1), Err(Refusal::NoLimit));
    let l = Limit { currency: "USD".into(), per_day_minor: 1000 };
    assert_eq!(check_spend(Some(&l), 0, "USD", 350), Ok(650));
    assert_eq!(check_spend(Some(&l), 900, "USD", 100), Ok(0), "exactly the limit is allowed");
    assert_eq!(check_spend(Some(&l), 900, "USD", 101), Err(Refusal::Over { remaining: 100, currency: "USD".into() }));
    assert_eq!(check_spend(Some(&l), 5000, "USD", 1), Err(Refusal::Over { remaining: 0, currency: "USD".into() }), "already over (the limit was lowered)");
    assert_eq!(check_spend(Some(&l), 0, "EUR", 1), Err(Refusal::OtherCurrency { allowed: "USD".into() }));
    for bad in [0, MAX_MINOR + 1] {
        assert_eq!(check_spend(Some(&l), 0, "USD", bad), Err(Refusal::BadAmount));
    }
    assert_eq!(check_spend(Some(&l), 0, "usd", 5), Err(Refusal::BadAmount));
    assert!(Refusal::Over { remaining: 100, currency: "USD".into() }.message().contains("1.00 USD"));
}

#[test]
fn amounts_are_written_with_the_currencys_own_decimals() {
    assert_eq!(format_minor(350, "USD"), "3.50 USD");
    assert_eq!(format_minor(5, "USD"), "0.05 USD");
    assert_eq!(format_minor(500, "JPY"), "500 JPY");
    assert_eq!(format_minor(1234, "KWD"), "1.234 KWD");
    assert_eq!((exponent("JPY"), exponent("USD"), exponent("BHD")), (0, 2, 3));
    assert!(valid_currency("USD") && !valid_currency("usd") && !valid_currency("US") && !valid_currency("USDD") && !valid_currency("U$D"));
}
