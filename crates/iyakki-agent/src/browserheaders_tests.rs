use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct Mem(Mutex<HashMap<String, String>>);
impl Values for Mem {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, v: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(id.into(), v.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bh-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rule(header: &str, domains: &str, prefix: &str) -> HeaderRule {
    HeaderRule { name: "Test rule".into(), domains: domains.into(), prefix: prefix.into(), header: header.into(), enabled: true, ..Default::default() }
}

fn url(s: &str) -> iyakki_web::Url {
    iyakki_web::Url::parse(s).unwrap()
}

fn store(tag: &str) -> (Store, Arc<Mem>, PathBuf) {
    let d = dir(tag);
    let m = Arc::new(Mem::default());
    (Store::new(&d.join("headers.json"), m.clone()), m, d)
}

#[test]
fn domain_and_prefix_filters_decide_who_gets_the_header() {
    let r = Compiled::new(&rule("X-Test", "*.corp.example, files.example.test", ""), "dummy-value".into()).unwrap();
    assert!(r.matches(&url("https://api.corp.example/x?q=1")));
    assert!(r.matches(&url("https://a.b.corp.example/")));
    assert!(r.matches(&url("https://files.example.test/")));
    assert!(!r.matches(&url("https://corp.example/")), "the bare name is not a subdomain");
    assert!(!r.matches(&url("https://evil-corp.example/")));
    assert!(!r.matches(&url("https://files.example.test.evil.test/")));
    let p = Compiled::new(&rule("X-Test", "api.example.test", "https://api.example.test/v1"), "dummy-value".into()).unwrap();
    assert!(p.matches(&url("https://api.example.test/v1/users")));
    assert!(!p.matches(&url("https://api.example.test/v2/users")));
    assert!(!p.matches(&url("https://api.example.test/v10")), "whole path segments");
    let only_prefix = Compiled::new(&rule("X-Test", "", "https://api.example.test/v1"), "dummy-value".into()).unwrap();
    assert!(only_prefix.matches(&url("https://api.example.test/v1/a")));
}

#[test]
fn plain_http_only_goes_to_this_computer_or_a_private_address() {
    let r = Compiled::new(&rule("X-Test", "example.test, 127.0.0.1, 192.168.1.5", ""), "dummy-value".into()).unwrap();
    assert!(!r.matches(&url("http://example.test/")));
    assert!(r.matches(&url("https://example.test/")));
    assert!(r.matches(&url("http://127.0.0.1:8080/")));
    assert!(r.matches(&url("http://192.168.1.5/")));
}

#[test]
fn a_redirect_off_the_site_is_judged_again_and_gets_nothing() {
    let rules = vec![Compiled::new(&rule("X-Test", "api.example.test", ""), "dummy-value".into()).unwrap()];
    assert_eq!(headers_for(&rules, &url("https://api.example.test/a")).len(), 1);
    assert!(headers_for(&rules, &url("https://elsewhere.example.test/a")).is_empty());
}

#[test]
fn forbidden_and_hop_by_hop_names_are_refused() {
    for n in ["Host", "content-length", "Transfer-Encoding", "Connection", "Upgrade", "Keep-Alive", "TE", "Proxy-Authorization", "Proxy-Foo", "Sec-Fetch-Mode", "Expect", "Accept-Encoding"] {
        assert!(check_name(n, true).is_err(), "{n}");
    }
    assert!(check_name("X-Api-Key", false).is_ok());
    assert!(check_name("Bad Name", false).is_err());
    assert!(check_name("A:B", false).is_err());
    assert!(check_name("", false).is_err());
}

#[test]
fn cookie_and_authorization_need_the_i_know_tick() {
    assert!(check_name("Authorization", false).is_err());
    assert!(check_name("cookie", false).is_err());
    assert!(check_name("Authorization", true).is_ok());
    let (s, _, _) = store("ack");
    assert!(s.add(rule("Authorization", "api.example.test", ""), "Bearer dummy").is_err());
    let mut r = rule("Authorization", "api.example.test", "");
    r.ack = true;
    assert!(s.add(r, "Bearer dummy").is_ok());
}

#[test]
fn values_with_line_breaks_or_control_characters_are_refused() {
    for v in ["a\r\nX-Evil: 1", "a\nb", "a\u{0}b", "a\u{7f}", "caf\u{e9}", "", "   "] {
        assert!(check_value(v).is_err(), "{v:?}");
    }
    assert!(check_value(&"x".repeat(MAX_VALUE + 1)).is_err());
    assert!(check_value("token with spaces\tand tab").is_ok());
}

#[test]
fn a_rule_must_name_its_sites_and_cannot_be_a_catch_all() {
    assert!(check_rule(&rule("X-A", "", "")).is_err());
    assert!(check_rule(&rule("X-A", "*", "")).is_err());
    assert!(check_rule(&rule("X-A", "*.com", "")).is_err());
    assert!(check_rule(&rule("X-A", "!a.example.test", "")).is_err());
    assert!(check_rule(&rule("X-A", "a.example.test", "not a url at all")).is_err());
    let mut r = rule("X-A", "a.example.test", "");
    r.name = " ".into();
    assert!(check_rule(&r).is_err());
}

#[test]
fn rules_persist_and_the_value_is_only_in_the_vault() {
    let (s, vault, d) = store("persist");
    let added = s.add(rule("X-Api-Key", "api.example.test", ""), "dummy-secret-value-123").unwrap();
    let file = std::fs::read_to_string(d.join("headers.json")).unwrap();
    assert!(!file.contains("dummy-secret-value-123"), "{file}");
    assert_eq!(vault.get(&added.id).unwrap().as_deref(), Some("dummy-secret-value-123"));
    // a new Store over the same file sees the rule
    let again = Store::new(&d.join("headers.json"), vault.clone());
    assert_eq!(again.list(), vec![added.clone()]);
    let (c, skipped) = again.compile();
    assert_eq!((c.len(), skipped), (1, 0));
    assert_eq!(headers_for(&c, &url("https://api.example.test/")), vec![("X-Api-Key".to_string(), "dummy-secret-value-123".to_string())]);
}

#[test]
fn enable_disable_and_delete() {
    let (s, vault, _) = store("life");
    let r = s.add(rule("X-A", "a.example.test", ""), "dummy-1").unwrap();
    assert_eq!(s.compile().0.len(), 1);
    s.set_enabled(&r.id, false).unwrap();
    assert!(!s.list()[0].enabled);
    assert_eq!(s.compile().0.len(), 0, "a disabled rule is kept but never applied");
    s.set_enabled(&r.id, true).unwrap();
    assert_eq!(s.compile().0.len(), 1);
    s.delete(&r.id).unwrap();
    assert!(s.list().is_empty());
    assert_eq!(vault.get(&r.id).unwrap(), None, "the value goes with the rule");
    assert!(s.delete(&r.id).is_err());
}

#[test]
fn update_keeps_the_old_value_unless_a_new_one_is_given() {
    let (s, vault, _) = store("upd");
    let r = s.add(rule("X-A", "a.example.test", ""), "dummy-1").unwrap();
    let mut edited = r.clone();
    edited.domains = "b.example.test".into();
    s.update(&r.id, edited.clone(), None).unwrap();
    assert_eq!(vault.get(&r.id).unwrap().as_deref(), Some("dummy-1"));
    s.update(&r.id, edited, Some("dummy-2")).unwrap();
    assert_eq!(vault.get(&r.id).unwrap().as_deref(), Some("dummy-2"));
    assert_eq!(s.list()[0].domains, "b.example.test");
}

#[test]
fn a_locked_vault_skips_the_rule_and_says_so() {
    struct Locked;
    impl Values for Locked {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("locked".into())
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let d = dir("locked");
    let s = Store::new(&d.join("headers.json"), Arc::new(Locked));
    s.add(rule("X-A", "a.example.test", ""), "dummy").unwrap();
    assert_eq!(s.compile().1, 1);
}

#[test]
fn first_rule_wins_per_header_name_and_the_count_is_capped() {
    let a = Compiled::new(&rule("X-A", "a.example.test", ""), "one".into()).unwrap();
    let b = Compiled::new(&rule("x-a", "a.example.test", ""), "two".into()).unwrap();
    assert_eq!(headers_for(&[a, b], &url("https://a.example.test/")), vec![("X-A".to_string(), "one".to_string())]);
    let many: Vec<Compiled> = (0..30).map(|i| Compiled::new(&rule(&format!("X-H{i}"), "a.example.test", ""), "v".into()).unwrap()).collect();
    assert_eq!(headers_for(&many, &url("https://a.example.test/")).len(), MAX_PER_REQUEST);
}

#[test]
fn values_never_appear_in_debug_text_or_the_mask() {
    let c = Compiled::new(&rule("X-A", "a.example.test", ""), "dummy-secret-xyz".into()).unwrap();
    assert!(!format!("{c:?}").contains("dummy-secret-xyz"));
    assert!(!masked("dummy-secret-xyz").contains("dummy"));
}

#[test]
fn the_rule_count_is_limited() {
    let (s, _, _) = store("max");
    for _ in 0..MAX_RULES {
        s.add(rule("X-A", "a.example.test", ""), "v").unwrap();
    }
    assert!(s.add(rule("X-A", "a.example.test", ""), "v").is_err());
}

#[test]
fn the_provider_reads_the_active_set() {
    let rules = vec![Compiled::new(&rule("X-Prov", "prov.example.test", ""), "dummy".into()).unwrap()];
    activate(rules);
    let p = provider();
    assert_eq!(p(&url("https://prov.example.test/")).len(), 1);
    assert_eq!(active_here(&url("https://prov.example.test/")), vec!["Test rule".to_string()]);
    activate(vec![]);
    assert!(p(&url("https://prov.example.test/")).is_empty());
}

#[test]
fn a_path_regex_narrows_a_rule_on_the_path_and_the_query_and_never_replaces_the_site_filter() {
    let mut r = rule("X-Test", "api.example.test", "");
    r.path_regex = "^/api/v[0-9]+/items".into();
    let c = Compiled::new(&r, "dummy-value".into()).unwrap();
    assert!(c.matches(&url("https://api.example.test/api/v2/items")));
    assert!(c.matches(&url("https://api.example.test/api/v10/items/7?x=1")));
    assert!(!c.matches(&url("https://api.example.test/api/vX/items")), "not a version number");
    assert!(!c.matches(&url("https://api.example.test/other")));
    assert!(!c.matches(&url("https://other.example.test/api/v2/items")), "the domain filter still applies");
    // the query is part of what is tested
    let mut q = rule("X-Test", "api.example.test", "");
    q.path_regex = r"\?debug=1$".into();
    let qc = Compiled::new(&q, "dummy-value".into()).unwrap();
    assert!(qc.matches(&url("https://api.example.test/a?debug=1")));
    assert!(!qc.matches(&url("https://api.example.test/a?debug=2")));
    assert!(!qc.matches(&url("https://api.example.test/a")));
    // an extension test
    let mut j = rule("X-Test", "*.example.test", "");
    j.path_regex = r"\.json$".into();
    let jc = Compiled::new(&j, "dummy-value".into()).unwrap();
    assert!(jc.matches(&url("https://a.example.test/data/list.json")));
    assert!(!jc.matches(&url("https://a.example.test/data/list.html")));
    // a regex alone sends the header to no site: the domain or prefix is still required
    let mut alone = rule("X-Test", "", "");
    alone.path_regex = ".*".into();
    assert!(check_rule(&alone).is_err(), "a pattern is not a site filter");
    // a broken pattern is refused in plain words, a runaway one cannot hang
    let mut bad = rule("X-Test", "api.example.test", "");
    bad.path_regex = "([a-z".into();
    assert!(check_rule(&bad).unwrap_err().contains("regular expression"));
    let mut slow = rule("X-Test", "api.example.test", "");
    slow.path_regex = "(a*)*b".into();
    let sc = Compiled::new(&slow, "dummy-value".into()).unwrap();
    assert!(!sc.matches(&url(&format!("https://api.example.test/{}", "a".repeat(200)))), "the step limit ends it");
    // an old rule file without the field still loads
    let old: HeaderRule = serde_json::from_str(r#"{"id":"1","name":"n","domains":"a.example.test","prefix":"","header":"X-Y","enabled":true,"ack":false}"#).unwrap();
    assert_eq!(old.path_regex, "");
}
