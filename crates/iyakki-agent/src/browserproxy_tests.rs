use super::*;
use std::path::PathBuf;

fn data(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bproxy-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_proxy_is_saved_checked_loaded_and_cleared_and_a_sign_in_is_refused() {
    let d = data("save");
    assert!(load(&d).is_none() && text(&d).is_empty());
    assert_eq!(save(&d, " http://proxy.example:3128 ").unwrap(), "proxy.example:3128");
    let p = load(&d).unwrap();
    assert!(iyakki_proxy::https::BrowserFetch::default().with_proxy(p.clone()).is_ok(), "what is saved always builds a client");
    assert_eq!((p.host.as_str(), p.port, p.creds), ("proxy.example", 3128, None));
    for bad in ["nonsense", "socks5://h:1", "ada:pw@h:1", "h:0"] {
        assert!(save(&d, bad).is_err(), "{bad}");
    }
    assert_eq!(text(&d), "proxy.example:3128", "a refused text changes nothing");
    assert_eq!(save(&d, "").unwrap(), "");
    assert!(load(&d).is_none());
    assert_eq!(save(&d, "  ").unwrap(), "", "clearing twice is fine");
    // a damaged or hand-edited file never gives a proxy with a sign-in or a bad address
    std::fs::write(d.join("browser-proxy.json"), r#"{"proxy":"ada:pw@h:1"}"#).unwrap();
    assert!(load(&d).is_none());
    std::fs::write(d.join("browser-proxy.json"), "not json").unwrap();
    assert!(load(&d).is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_tools_say_what_is_set_ask_for_the_change_and_exist_only_with_the_settings_and_the_browser() {
    let d = data("tools");
    assert!(run("browser_proxy_status", &json!({}), &d).unwrap().contains("no proxy"));
    assert!(run("browser_proxy_set", &json!({}), &d).is_err());
    assert!(run("browser_proxy_set", &json!({"proxy": "px:8080"}), &d).unwrap().contains("px:8080"));
    assert!(run("browser_proxy_status", &json!({}), &d).unwrap().contains("px:8080"));
    assert!(run("browser_proxy_set", &json!({"proxy": ""}), &d).unwrap().contains("off"));
    let all = iyakki_sec::policy::Role::Admin.template();
    let t = defs(&all);
    assert_eq!(t.iter().map(|x| (x.spec.name.as_str(), x.risk)).collect::<Vec<_>>(), vec![("browser_proxy_status", Risk::Safe), ("browser_proxy_set", Risk::Ask)]);
    let mut no = all.clone();
    no.apps.remove(&App::Browser);
    assert!(defs(&no).is_empty());
    assert!(describe("browser_proxy_set", &json!({"proxy": "px:1"})).unwrap().contains("px:1"));
    assert!(iyakki_shared::toolpolicy::hard_ask("browser_proxy_set", &json!({"proxy": "px:1"})).is_some());
    let _ = std::fs::remove_dir_all(&d);
}
