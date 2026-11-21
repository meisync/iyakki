use super::*;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-plugins-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn script(name: &str, sites: &[&str]) -> Plugin {
    Plugin { id: String::new(), name: name.into(), kind: Kind::Script, sites: sites.iter().map(|s| s.to_string()).collect(), body: "document.title = 'x';".into(), at_end: false, enabled: false }
}

fn ua(name: &str, s: &str) -> Plugin {
    Plugin { id: String::new(), name: name.into(), kind: Kind::UserAgent, sites: vec![], body: s.into(), at_end: false, enabled: false }
}

#[test]
fn there_are_no_plugins_until_one_is_added_and_a_new_one_is_off() {
    let d = dir("none");
    let s = PluginStore::open(&d.join("p.json"));
    assert!(s.list().is_empty());
    let p = s.add(script("Dark", &["example.com"])).unwrap();
    assert!(!p.enabled && p.id == "pl1");
    assert!(hooks_for(&s.list()).is_empty(), "a plug-in that is off gives the engine nothing");
}

#[test]
fn a_script_or_style_must_name_sites_and_never_a_whole_domain_ending_or_every_site() {
    let d = dir("sites");
    let s = PluginStore::open(&d.join("p.json"));
    for bad in [vec![], vec!["*"], vec!["*.com"], vec!["com"], vec!["a b.com"], vec!["-x.com"], vec!["ex..com"], vec!["https://example.com"]] {
        assert!(s.add(script("S", &bad)).is_err(), "{bad:?}");
    }
    assert!(s.add(script("S", &["example.com", "*.example.org", "localhost"])).is_ok());
    let nine = vec!["a.com"; 9];
    assert!(s.add(script("T", &nine)).is_err());
}

#[test]
fn the_user_agent_has_no_sites_one_line_of_printable_text_and_only_one_is_on_at_a_time() {
    let d = dir("ua");
    let s = PluginStore::open(&d.join("p.json"));
    assert!(s.add(ua("U", "short")).is_err());
    assert!(s.add(ua("U", "Mozilla/5.0\r\nX: y")).is_err());
    let mut with_sites = ua("U", "Mozilla/5.0 (X11; Linux x86_64)");
    with_sites.sites = vec!["example.com".into()];
    assert!(s.add(with_sites).is_err());
    let a = s.add(ua("A", "Mozilla/5.0 (X11; Linux x86_64) A")).unwrap();
    let b = s.add(ua("B", "Mozilla/5.0 (X11; Linux x86_64) B")).unwrap();
    s.set_enabled(&a.id, true).unwrap();
    s.set_enabled(&b.id, true).unwrap();
    let on: Vec<String> = s.list().into_iter().filter(|p| p.enabled).map(|p| p.name).collect();
    assert_eq!(on, vec!["B"]);
}

#[test]
fn what_is_on_becomes_a_content_script_hook_and_a_user_agent_hook_that_decode_back() {
    let d = dir("hooks");
    let s = PluginStore::open(&d.join("p.json"));
    let a = s.add(script("Dark", &["example.com", "*.example.org"])).unwrap();
    let mut st = script("Wide", &["example.com"]);
    st.kind = Kind::Style;
    st.body = "body { color: red }".into();
    st.at_end = true;
    let b = s.add(st).unwrap();
    let u = s.add(ua("UA", "Mozilla/5.0 (X11; Linux x86_64) Test")).unwrap();
    s.set_enabled(&a.id, true).unwrap();
    s.set_enabled(&b.id, true).unwrap();
    s.set_enabled(&u.id, true).unwrap();
    let hs = hooks_for(&s.list());
    assert_eq!(hs.iter().map(|h| h.kind).collect::<Vec<_>>(), vec![hook::CONTENT_SCRIPT, hook::USER_AGENT]);
    let list = ext::decode_content_scripts(&hs[0].payload).unwrap();
    assert_eq!(list.len(), 2);
    assert!(!list[0].style && !list[0].at_end && list[0].domains.len() == 2 && list[0].domains[1].suffix);
    assert!(list[1].style && list[1].at_end);
    assert_eq!(ext::decode_user_agent(&hs[1].payload).unwrap(), "Mozilla/5.0 (X11; Linux x86_64) Test");
    s.set_enabled(&a.id, false).unwrap();
    assert_eq!(ext::decode_content_scripts(&hooks_for(&s.list())[0].payload).unwrap().len(), 1);
}

#[test]
fn the_list_is_kept_in_the_file_and_a_damaged_or_hand_edited_bad_entry_is_dropped() {
    let d = dir("file");
    let path = d.join("p.json");
    {
        let s = PluginStore::open(&path);
        let a = s.add(script("Keep", &["example.com"])).unwrap();
        s.set_enabled(&a.id, true).unwrap();
    }
    let s = PluginStore::open(&path);
    assert_eq!(s.list().len(), 1);
    assert!(s.find("keep").unwrap().enabled);
    // an entry that names every site is not read
    let mut v = s.list();
    v.push(Plugin { id: "pl9".into(), sites: vec![], ..script("Evil", &[]) });
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(PluginStore::open(&path).list().len(), 1);
    std::fs::write(&path, "{not json").unwrap();
    assert!(PluginStore::open(&path).list().is_empty());
}

#[test]
fn limits_hold_for_the_count_the_size_and_a_second_plugin_of_the_same_name() {
    let d = dir("limits");
    let s = PluginStore::open(&d.join("p.json"));
    s.add(script("Same", &["example.com"])).unwrap();
    assert!(s.add(script("same", &["example.com"])).is_err());
    let mut big = script("Big", &["example.com"]);
    big.body = "x".repeat(ext::MAX_BODY + 1);
    assert!(s.add(big).is_err());
    for i in 0..15 {
        s.add(script(&format!("n{i}"), &["example.com"])).unwrap();
    }
    assert!(s.add(script("one too many", &["example.com"])).is_err());
    assert_eq!(s.remove("pl1").unwrap().name, "Same");
    assert!(s.add(script("room again", &["example.com"])).is_ok());
}

#[test]
fn the_extension_asks_only_for_the_capabilities_of_what_is_on_and_activation_tells_the_host() {
    let d = dir("ext");
    let s = PluginStore::open(&d.join("p.json"));
    let a = s.add(script("A", &["example.com"])).unwrap();
    let u = s.add(ua("U", "Mozilla/5.0 (X11; Linux x86_64) U")).unwrap();
    let g0 = crate::browserext::host().generation();
    s.set_enabled(&u.id, true).unwrap();
    activate(s.list());
    assert!(crate::browserext::host().generation() > g0);
    assert_eq!(PluginsExtension.capabilities(), vec!["web.user.agent".to_string()]);
    s.set_enabled(&a.id, true).unwrap();
    activate(s.list());
    assert_eq!(PluginsExtension.capabilities(), vec!["web.content.script".to_string(), "web.user.agent".to_string()]);
    let snap = crate::browserext::host().snapshot();
    let e = snap.entries.iter().find(|e| e.id == PLUGINS_ID).expect("the plug-ins are in the snapshot once the broker holds their capabilities");
    assert_eq!(e.hooks.iter().map(|h| h.kind).collect::<Vec<_>>(), vec![hook::CONTENT_SCRIPT, hook::USER_AGENT]);
    activate(Vec::new());
    assert!(crate::browserext::host().snapshot().entries.iter().all(|e| e.id != PLUGINS_ID), "nothing on: nothing sent");
    assert!(PluginsExtension.capabilities().is_empty() && PluginsExtension.hooks().is_empty());
}
