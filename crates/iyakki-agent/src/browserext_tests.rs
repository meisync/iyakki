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
    let _rules = iyakki_testkit::process_lock(iyakki_testkit::names::BROWSER_HEADERS);
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
    let s = mk(&["web.request.headers"], vec![Hook { kind: 777, payload: vec![1] }, Hook { kind: hook::PAGE_ACTION, payload: vec![] }, good.clone()]);
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

// ---- the helper's half: user scripts, user styles and the User-Agent (F-BROW-90) --------------------------------------------------

fn run_content_c(tag: &str, snap: &[u8]) -> (bool, String) {
    let cc = cc().expect("a C compiler");
    let dir = std::env::temp_dir().join(format!("iyakki-contentc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper/content-check.c");
    let exe = dir.join("content-check");
    let out = std::process::Command::new(cc).args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-parameter", "-o"]).arg(&exe).arg(&src).output().unwrap();
    assert!(out.status.success(), "content-check.c does not compile: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(dir.join("snap.bin"), snap).unwrap();
    let o = std::process::Command::new(&exe).arg(dir.join("snap.bin")).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    (o.status.success(), text)
}

fn pat(host: &str, suffix: bool) -> ext::Pat {
    ext::Pat { scheme: 0, suffix, host: host.into(), port: 0, segs: vec![] }
}

fn content_snapshot(caps: &[&str], hooks: Vec<Hook>) -> Vec<u8> {
    Snapshot { entries: vec![Entry { id: "browser-plugins".into(), version: "1".into(), caps: caps.iter().map(|s| s.to_string()).collect(), hooks }] }.encode().0
}

#[test]
fn the_helper_reads_scripts_styles_and_the_user_agent_from_the_snapshot_and_turns_sites_into_engine_patterns() {
    let list = vec![
        ext::ContentScript { style: false, at_end: true, domains: vec![pat("example.com", false)], body: "document.title='x'".into() },
        ext::ContentScript { style: true, at_end: false, domains: vec![pat("example.org", true)], body: "a{color:red}".into() },
        ext::ContentScript { style: false, at_end: false, domains: vec![pat("both.test", false), pat("both.test", true)], body: "1".into() },
        ext::ContentScript { style: false, at_end: false, domains: vec![ext::Pat { scheme: 2, suffix: false, host: "path.test".into(), port: 0, segs: vec!["a".into(), "b".into()] }], body: "2".into() },
    ];
    let snap = content_snapshot(&["web.content.script", "web.user.agent"], vec![Hook { kind: hook::CONTENT_SCRIPT, payload: ext::encode_content_scripts(&list) }, Hook { kind: hook::USER_AGENT, payload: ext::encode_user_agent("Mozilla/5.0 (Test) Gecko/1") }]);
    let (ok, out) = run_content_c("good", &snap);
    assert!(ok, "{out}");
    assert!(out.contains("parse 0, 4 scripts"), "{out}");
    // a bare site: both schemes, whole site
    assert!(out.contains("script end 18 allow=http://example.com/*,https://example.com/* nullterm=1"), "{out}");
    // *.example.org: the domain and its subdomains, which is what the engine's `*.` pattern matches
    assert!(out.contains("style start 12 allow=http://*.example.org/*,https://*.example.org/* nullterm=1"), "{out}");
    assert!(out.contains("allow=http://both.test/*,https://both.test/*,http://*.both.test/*,https://*.both.test/* nullterm=1"), "{out}");
    // one scheme and a path
    assert!(out.contains("allow=https://path.test/a/b/* nullterm=1"), "{out}");
    assert!(out.contains("ua=Mozilla/5.0 (Test) Gecko/1"), "{out}");
}

#[test]
fn a_script_without_a_usable_site_a_port_a_missing_capability_or_a_bad_user_agent_is_ignored_and_named() {
    let with_port = ext::ContentScript { style: false, at_end: false, domains: vec![ext::Pat { scheme: 0, suffix: false, host: "p.test".into(), port: 8443, segs: vec![] }], body: "1".into() };
    let no_site = ext::ContentScript { style: false, at_end: false, domains: vec![], body: "1".into() };
    let empty = ext::ContentScript { style: false, at_end: false, domains: vec![pat("e.test", false)], body: String::new() };
    let good = ext::ContentScript { style: false, at_end: false, domains: vec![pat("g.test", false)], body: "3".into() };
    let h = vec![Hook { kind: hook::CONTENT_SCRIPT, payload: ext::encode_content_scripts(&[with_port, no_site, empty, good]) }];
    let (ok, out) = run_content_c("drop", &content_snapshot(&["web.content.script"], h.clone()));
    assert!(ok, "{out}");
    assert!(out.contains("parse 0, 1 scripts") && out.contains("3 script or style without a usable site"), "{out}");
    // never a catch-all: nothing in the output allows every site
    assert!(!out.contains("allow=*") && !out.contains("://*/"), "{out}");
    // without the capability the hook is ignored
    let (ok, out) = run_content_c("nocap", &content_snapshot(&["web.request.headers"], h));
    assert!(ok && out.contains("0 scripts") && out.contains("without the capability web.content.script"), "{out}");
    // a User-Agent with a control character, or too long, is broken and ignored
    for bad in ["Mozilla\nX-Evil: 1", &"a".repeat(401)] {
        let h = vec![Hook { kind: hook::USER_AGENT, payload: { let mut w = (bad.len() as u32).to_le_bytes().to_vec(); w.extend_from_slice(bad.as_bytes()); w } }];
        let (ok, out) = run_content_c("badua", &content_snapshot(&["web.user.agent"], h));
        assert!(ok && out.contains("ua=-") && out.contains("user.agent is broken"), "{out}");
    }
    // an empty User-Agent means the engine's own
    let (ok, out) = run_content_c("emptyua", &content_snapshot(&["web.user.agent"], vec![Hook { kind: hook::USER_AGENT, payload: ext::encode_user_agent("") }]));
    assert!(ok && out.contains("ua=(engine's own)"), "{out}");
    // a cut-short script payload is broken, nothing half read is kept
    let mut p = ext::encode_content_scripts(&[ext::ContentScript { style: false, at_end: false, domains: vec![pat("g.test", false)], body: "x".repeat(100) }]);
    p.truncate(p.len() - 10);
    let (ok, out) = run_content_c("cut", &content_snapshot(&["web.content.script"], vec![Hook { kind: hook::CONTENT_SCRIPT, payload: p }]));
    assert!(ok && out.contains("0 scripts") && out.contains("content.script is broken"), "{out}");
}

#[test]
fn what_the_plug_in_registry_hands_over_is_what_the_helper_reads_back() {
    use iyakki_shared::browserplugins as bp;
    // the registry's own hooks (scripts, styles, one User-Agent) for plug-ins that are on, read by the helper's C parser
    let mk = |id: &str, kind: bp::Kind, sites: &[&str], body: &str, enabled: bool| bp::Plugin { id: id.into(), name: id.into(), kind, sites: sites.iter().map(|s| s.to_string()).collect(), body: body.into(), at_end: false, enabled };
    let list = vec![
        mk("s1", bp::Kind::Script, &["example.com", "*.example.org"], "console.log(1)", true),
        mk("st", bp::Kind::Style, &["example.com"], "body{background:#eee}", true),
        mk("off", bp::Kind::Script, &["off.test"], "1", false),
        mk("ua", bp::Kind::UserAgent, &[], "Mozilla/5.0 Registry", true),
    ];
    let hooks = bp::hooks_for(&list);
    let (ok, out) = run_content_c("registry", &content_snapshot(&["web.content.script", "web.user.agent"], hooks));
    assert!(ok, "{out}");
    assert!(out.contains("parse 0, 2 scripts") && out.contains("ua=Mozilla/5.0 Registry"), "{out}");
}

// ---- the media policy (F-BROX-18, F-BROX-20): the page script in a real script engine and the helper's pure parts in C -----------------

/// Run the helper's media-cap.js in the reader's real script engine with stand-ins for the three media APIs it wraps (they answer "yes" to everything), then `probe`.
fn media_cap(h: u32, codecs: u32, probe: &str) -> String {
    let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper/media-cap.js")).unwrap();
    let url = iyakki_web::Url::parse("https://a.org/").unwrap();
    let web: Arc<dyn iyakki_web::Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
    let out = std::sync::Mutex::new(String::new());
    let stand_ins = "globalThis.MediaSource = { isTypeSupported: function (t) { return true; } };\
        globalThis.HTMLMediaElement = function () {}; HTMLMediaElement.prototype.canPlayType = function (t) { return 'probably'; };\
        navigator.mediaCapabilities = { decodingInfo: function (c) { return Promise.resolve({ supported: true, smooth: true, powerEfficient: false }); } };";
    iyakki_js::run_page_with("<body></body>", &url, web, |_| true, &iyakki_js::SiteData::default(), iyakki_js::Limits::default(), |e| {
        let prefix = format!("window.__iyakkiCap={{h:{h},c:{codecs}}};");
        let r = e.eval("cap", &format!("{stand_ins}{prefix}{src}\n;{probe}")).unwrap_or_else(|x| format!("THROWN {x}"));
        let _ = e.run_jobs();
        let later = e.eval("later", "typeof __later === 'undefined' ? '' : String(__later)").unwrap_or_default();
        *out.lock().unwrap() = if later.is_empty() { r } else { later };
    });
    let s = out.lock().unwrap().clone();
    s
}

#[test]
fn a_video_ceiling_makes_the_page_see_taller_pictures_as_unsupported_and_nothing_else_changes() {
    let probe = "[MediaSource.isTypeSupported('video/webm; codecs=\"vp9\"; width=1920; height=1080'), MediaSource.isTypeSupported('video/webm; codecs=\"vp9\"; width=1280; height=720'), MediaSource.isTypeSupported('video/webm; codecs=\"vp9\"'), new HTMLMediaElement().canPlayType('video/mp4; height=2160'), new HTMLMediaElement().canPlayType('video/mp4; height=480'), MediaSource.isTypeSupported('audio/webm; codecs=opus')].join()";
    assert_eq!(media_cap(720, 1, probe), "false,true,true,,probably,true");
    // no ceiling: every answer is the engine's own
    assert_eq!(media_cap(0, 1, probe), "true,true,true,probably,probably,true");
    // the ceiling changes while the page is open: the wrappers read the object at every call
    let live = "var before = MediaSource.isTypeSupported('video/mp4; height=1080'); window.__iyakkiCap = { h: 480, c: 1 }; [before, MediaSource.isTypeSupported('video/mp4; height=1080'), MediaSource.isTypeSupported('video/mp4; height=360')].join()";
    assert_eq!(media_cap(0, 1, live), "true,false,true");
    // a ceiling of 480 with a 480 picture is allowed, one pixel more is not
    assert_eq!(media_cap(480, 1, "[MediaSource.isTypeSupported('video/mp4; height=480'), MediaSource.isTypeSupported('video/mp4; height=481')].join()"), "true,false");
}

#[test]
fn the_add_on_decoders_off_makes_h264_h265_aac_and_ac3_unsupported_and_leaves_vp9_opus_and_mp3_alone() {
    let probe = "['video/mp4; codecs=\"avc1.64001f\"', 'video/mp4; codecs=\"hev1.1.6.L93.B0\"', 'audio/mp4; codecs=\"mp4a.40.2\"', 'audio/mp4; codecs=\"ec-3\"', 'video/webm; codecs=\"vp9\"', 'audio/webm; codecs=\"opus\"', 'audio/mpeg', 'audio/mp4; codecs=\"mp4a.6b\"'].map(function (t) { return MediaSource.isTypeSupported(t) ? 1 : 0; }).join('')";
    assert_eq!(media_cap(0, 0, probe), "00001111");
    assert_eq!(media_cap(0, 1, probe), "11111111");
    // the capabilities API answers the same way, with the size and the codec of its configuration
    let caps = "navigator.mediaCapabilities.decodingInfo({ type: 'media-source', video: { contentType: 'video/mp4; codecs=\"avc1.64001f\"', width: 1280, height: 720 } }).then(function (r) { var a = r.supported; return navigator.mediaCapabilities.decodingInfo({ type: 'media-source', video: { contentType: 'video/webm; codecs=\"vp9\"', width: 1280, height: 720 } }).then(function (r2) { globalThis.__later = [a, r2.supported].join(); }); }); 'pending'";
    assert_eq!(media_cap(0, 0, caps), "false,true");
    let tall = "navigator.mediaCapabilities.decodingInfo({ video: { contentType: 'video/webm; codecs=\"vp9\"', width: 3840, height: 2160 } }).then(function (r) { globalThis.__later = String(r.supported); }); 'pending'";
    assert_eq!(media_cap(1080, 1, tall), "false");
    assert_eq!(media_cap(2160, 1, tall), "true");
}

#[test]
fn the_helpers_media_policy_lines_are_digits_only_and_the_decoder_rank_list_keeps_what_the_helper_started_with() {
    let cc = cc().expect("a C compiler");
    let dir = std::env::temp_dir().join(format!("iyakki-policyc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper/policy-check.c");
    let exe = dir.join("policy-check");
    let out = std::process::Command::new(cc).args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-o"]).arg(&exe).arg(&src).output().unwrap();
    assert!(out.status.success(), "policy-check.c does not compile: {}", String::from_utf8_lossy(&out.stderr));
    let run = |args: &[&str]| String::from_utf8_lossy(&std::process::Command::new(&exe).args(args).output().unwrap().stdout).to_string();
    assert_eq!(run(&["720", "1"]), "window.__iyakkiCap={h:720,c:1};\nrank 0 \n");
    // a ceiling nobody could mean is no ceiling
    assert_eq!(run(&["99999", "1"]), "window.__iyakkiCap={h:0,c:1};\nrank 0 \n");
    assert_eq!(run(&["0", "7"]), "window.__iyakkiCap={h:0,c:1};\nrank 0 \n");
    let off = run(&["480", "0", "alsadeviceprovider:0"]);
    assert!(off.starts_with("window.__iyakkiCap={h:480,c:0};\nrank 0 alsadeviceprovider:0,avdec_h264:0,avdec_h265:0,avdec_aac:0,"), "{off}");
    assert!(off.contains("avdec_ac3:0") && off.trim_end().ends_with("avdec_mpeg4:0"), "{off}");
    // decoders on again: the helper's own value comes back whole
    assert_eq!(run(&["0", "1", "alsadeviceprovider:0"]), "window.__iyakkiCap={h:0,c:1};\nrank 0 alsadeviceprovider:0\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_helpers_cache_limit_adds_up_the_folder_and_says_over_only_above_the_limit() {
    let cc = cc().expect("a C compiler");
    let dir = std::env::temp_dir().join(format!("iyakki-cachec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tree/a/b")).unwrap();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper/cache-check.c");
    let exe = dir.join("cache-check");
    let out = std::process::Command::new(cc).args(["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-o"]).arg(&exe).arg(&src).output().unwrap();
    assert!(out.status.success(), "cache-check.c does not compile: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::write(dir.join("tree/a/b/big"), vec![0u8; 1024 * 1024 + 5]).unwrap();
    std::fs::write(dir.join("tree/small"), b"1234567890").unwrap();
    // a link out of the folder is not followed (its target is not counted)
    #[cfg(unix)]
    std::os::unix::fs::symlink(&dir, dir.join("tree/loop")).unwrap();
    let run = |limit: &str| String::from_utf8_lossy(&std::process::Command::new(&exe).arg(dir.join("tree")).arg(limit).output().unwrap().stdout).to_string();
    assert!(run("1").starts_with("bytes 1048591 entries 5 cut 0 over 1 limit 1"), "{}", run("1"));
    assert!(run("2").contains("over 0 limit 2"), "{}", run("2"));
    assert!(run("0").contains("over 0 limit 0"), "0 is no limit");
    assert!(run("70000").contains("over 0 limit 0"), "a limit nobody could mean is no limit");
    // a folder that is not there counts nothing and is never over
    let o = std::process::Command::new(&exe).arg(dir.join("none")).arg("1").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("bytes 0 "));
    let _ = std::fs::remove_dir_all(&dir);
}
