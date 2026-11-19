use iyakki_shared::browserext::*;
use crate::browserheaders::{check_rule, Compiled, HeaderRule};
use iyakki_webview::ext::{self, hook, Entry, Hook, Snapshot};
use std::sync::Arc;

fn rule(header: &str, domains: &str, prefix: &str) -> HeaderRule {
    HeaderRule { name: "Test rule".into(), domains: domains.into(), prefix: prefix.into(), header: header.into(), enabled: true, ..Default::default() }
}
fn compiled(header: &str, value: &str, domains: &str, prefix: &str) -> Compiled {
    let r = rule(header, domains, prefix);
    Compiled::new(&r, value.into()).unwrap_or_else(|| panic!("{:?}", check_rule(&r)))
}

struct Fixed(Vec<&'static str>, Vec<Hook>, &'static str);
impl BrowserExtension for Fixed {
    fn id(&self) -> &str {
        self.2
    }
    fn version(&self) -> &str {
        "1"
    }
    fn capabilities(&self) -> Vec<String> {
        self.0.iter().map(|s| s.to_string()).collect()
    }
    fn hooks(&self) -> Vec<Hook> {
        self.1.clone()
    }
}

fn some_hook() -> Hook {
    let c = compiled("X-A", "v", "a.example", "");
    Hook { kind: hook::REQUEST_HEADERS, payload: ext::encode_header_rules(&[c.to_wire().unwrap()]) }
}

#[test]
fn an_extension_without_its_capability_gets_no_hook() {
    let h = BrowserExtensionHost::new(Arc::new(BrokerGate::builtin("alice", "box")));
    // not in the built-in table: the broker says not_granted, so the entry is left out whole
    h.register(Arc::new(Fixed(vec!["web.request.headers"], vec![some_hook()], "stranger")));
    assert!(h.snapshot().entries.is_empty());
    // the header rules extension is in the table
    h.register(Arc::new(Fixed(vec!["web.request.headers"], vec![some_hook()], HEADERS_ID)));
    let s = h.snapshot();
    assert_eq!(s.entries.len(), 1);
    assert_eq!(s.entries[0].caps, ["web.request.headers"]);
    // a hook kind whose capability it never asked for is dropped even for a known extension
    h.register(Arc::new(Fixed(vec!["web.request.headers"], vec![some_hook(), Hook { kind: hook::CONTENT_SCRIPT, payload: vec![] }], HEADERS_ID)));
    assert_eq!(h.snapshot().entries[0].hooks.len(), 1);
}

#[test]
fn a_gate_that_says_no_removes_the_hook() {
    struct No;
    impl CapGate for No {
        fn allows(&self, _: &str, _: &str) -> bool {
            false
        }
    }
    let h = BrowserExtensionHost::new(Arc::new(No));
    h.register(Arc::new(Fixed(vec!["web.request.headers"], vec![some_hook()], HEADERS_ID)));
    assert!(h.snapshot().entries.is_empty());
    let g = h.generation();
    h.set_gate(Arc::new(BrokerGate::builtin("", "")));
    assert!(h.generation() > g && h.snapshot().entries.len() == 1);
}

#[test]
fn the_header_rules_are_the_first_builtin_extension_and_follow_activate() {
    crate::browserheaders::activate(vec![compiled("X-Token", "s3cr3t-value", "api.example.org", "")]);
    let (gen1, blob) = host().blob();
    let snap = Snapshot::decode(&blob.0).unwrap();
    let d = snap.dispatch();
    assert_eq!(d.header_rules.len(), 1);
    assert_eq!(d.header_rules[0].header, "X-Token");
    assert!(d.report.is_empty());
    assert!(!format!("{blob:?} {:?} {:?} {:?}", host(), d, snap).contains("s3cr3t"));
    crate::browserheaders::activate(vec![]);
    let (gen2, blob2) = host().blob();
    assert!(gen2 > gen1);
    assert!(Snapshot::decode(&blob2.0).unwrap().entries.is_empty(), "no rules, no hook, no entry");
}

#[test]
fn rules_the_full_engine_cannot_express_are_not_sent_there() {
    let mut r = rule("X-A", "api.example.org", "");
    r.path_regex = "^/v1/".into();
    assert!(Compiled::new(&r, "v".into()).unwrap().to_wire().is_none(), "a path regex stays in-house");
    assert!(compiled("X-A", "v", "203.0.113.0/24", "").to_wire().is_none(), "an address range stays in-house");
    assert!(compiled("X-A", "v", "api.example.org, *.corp.example", "https://example.org/api").to_wire().is_some());
}

/// Rules, then (url, header names expected). The in-house matcher, the Rust twin over the wire and the C twin must all agree.
fn vectors() -> (Vec<Compiled>, Vec<(&'static str, Vec<&'static str>)>) {
    let rules = vec![
        compiled("X-Api", "k1", "api.example.org, *.corp.example", ""),
        compiled("X-Path", "k2", "", "https://example.org/api/v2"),
        compiled("X-Port", "k4", "https://port.example:8443", ""),
        compiled("X-Dup", "first", "dup.example", ""),
        compiled("x-dup", "second", "dup.example", ""),
    ];
    let cases = vec![
        ("https://api.example.org/x?q=1", vec!["X-Api"]),
        ("https://API.example.org:443/", vec!["X-Api"]),
        ("https://a.b.corp.example/z", vec!["X-Api"]),
        ("https://corp.example/", vec![]),
        ("https://evil-corp.example/", vec![]),
        ("https://api.example.org.evil.test/", vec![]),
        ("http://api.example.org/x", vec![]),
        ("https://example.org/api/v2", vec!["X-Path"]),
        ("https://example.org/api/v2/items?id=7", vec!["X-Path"]),
        ("https://example.org/api/v3", vec![]),
        ("https://example.org/api", vec![]),
        ("http://example.org/api/v2", vec![]),
        ("http://localhost:8080/", vec![]),
        ("https://localhost/", vec![]),
        ("https://port.example:8443/a", vec!["X-Port"]),
        ("https://port.example/a", vec![]),
        ("https://dup.example/", vec!["X-Dup"]),
        ("https://other.example/", vec![]),
    ];
    (rules, cases)
}

#[test]
fn the_in_house_matcher_and_the_wire_twin_agree_on_every_vector() {
    let (rules, cases) = vectors();
    let wire: Vec<_> = rules.iter().map(|r| r.to_wire().unwrap()).collect();
    for (u, want) in cases {
        let url = iyakki_web::Url::parse(u).unwrap();
        let inhouse: Vec<String> = crate::browserheaders::headers_for(&rules, &url).into_iter().map(|h| h.0).collect();
        let t = ext::Target { https: url.https, host: url.host.clone(), port: url.port.unwrap_or(if url.https { 443 } else { 80 }), segs: url.path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect(), ambiguous: url.path.contains(['%', '\\']) };
        let twin: Vec<String> = ext::headers_for(&wire, &t).into_iter().map(|h| h.0.to_string()).collect();
        assert_eq!(inhouse, want, "in-house {u}");
        assert_eq!(twin, want, "wire twin {u}");
    }
}

fn cc() -> Option<std::path::PathBuf> {
    ["cc", "gcc", "clang"].iter().find_map(|c| std::process::Command::new(c).arg("--version").output().ok().filter(|o| o.status.success()).map(|_| std::path::PathBuf::from(c)))
}

fn run_c(tag: &str, snap: &[u8], cases: &str, report: Option<&str>) -> (bool, String) {
    let cc = cc().expect("a C compiler");
    let dir = std::env::temp_dir().join(format!("iyakki-extc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper/ext-check.c");
    let exe = dir.join("ext-check");
    let out = std::process::Command::new(cc).args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-parameter", "-o"]).arg(&exe).arg(&src).output().unwrap();
    assert!(out.status.success(), "ext-check.c does not compile: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(dir.join("snap.bin"), snap).unwrap();
    std::fs::write(dir.join("cases.txt"), cases).unwrap();
    let mut c = std::process::Command::new(&exe);
    c.arg(dir.join("snap.bin")).arg(dir.join("cases.txt"));
    if let Some(r) = report {
        c.arg(r);
    }
    let o = c.output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    (o.status.success(), text)
}

#[test]
fn the_c_matcher_of_the_web_process_extension_agrees_with_the_rust_side() {
    let (rules, cases) = vectors();
    let mut text = String::new();
    for (u, want) in &cases {
        text += &format!("{u}\t{}\n", if want.is_empty() { "-".to_string() } else { want.join(",") });
    }
    // cases only the stricter C side decides (it never sends when a path is ambiguous or an address is not a plain name)
    text += "https://example.org/api/v2/../v3\t-\nhttps://example.org/api/v2/%2e%2e/x\t-\nhttps://user@api.example.org/\t-\nhttps://api.example.org./\t-\nftp://api.example.org/\t-\nhttps://[::1]/\t-\n";
    let wire: Vec<_> = rules.iter().map(|r| r.to_wire().unwrap()).collect();
    let h = BrowserExtensionHost::new(Arc::new(BrokerGate::builtin("", "")));
    h.register(Arc::new(Fixed(vec!["web.request.headers"], vec![Hook { kind: hook::REQUEST_HEADERS, payload: ext::encode_header_rules(&wire) }], HEADERS_ID)));
    let blob = h.blob().1;
    let (ok, out) = run_c("agree", &blob.0, &text, None);
    assert!(ok, "{out}");
    assert!(out.contains("0 failed") && out.contains("5 rules"), "{out}");
}

#[test]
fn the_c_side_ignores_unknown_hooks_refuses_other_versions_and_reports_without_values() {
    let c = compiled("X-A", "s3cr3t-value", "a.example", "");
    let good = Hook { kind: hook::REQUEST_HEADERS, payload: ext::encode_header_rules(&[c.to_wire().unwrap()]) };
    let mk = |caps: &[&str], hooks: Vec<Hook>| Snapshot { entries: vec![Entry { id: "browser-headers".into(), version: "1".into(), caps: caps.iter().map(|s| s.to_string()).collect(), hooks }] }.encode().0;
    // an unknown kind and a reserved kind are named and skipped; the good hook still works
    let s = mk(&["web.request.headers"], vec![Hook { kind: 777, payload: vec![1] }, Hook { kind: hook::CONTENT_SCRIPT, payload: vec![] }, good.clone()]);
    let (ok, out) = run_c("unknown", &s, "https://a.example/\tX-A\nhttps://b.example/\t-\n", Some("kind 777 is unknown"));
    assert!(ok, "{out}");
    assert!(out.contains("not implemented") && !out.contains("s3cr3t"), "{out}");
    // without the capability the hook is ignored
    let s = mk(&["notify"], vec![good.clone()]);
    let (ok, out) = run_c("nocap", &s, "https://a.example/\t-\n", Some("without the capability"));
    assert!(ok, "{out}");
    // another layout version is refused whole
    let mut s = mk(&["web.request.headers"], vec![good.clone()]);
    s[0] = 9;
    let (ok, out) = run_c("version", &s, "https://a.example/\t-\n", Some("version 9 is not supported"));
    assert!(ok, "{out}");
    // truncated bytes are refused, not read past
    let s = mk(&["web.request.headers"], vec![good]);
    let (ok, out) = run_c("cut", &s[..s.len() - 5], "https://a.example/\t-\n", Some("cut short"));
    assert!(ok, "{out}");
}
