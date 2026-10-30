//! Diagnosis through developer mode without a root shell: the `kube` scope (verb allow-list, Secrets blocked, caps, audit), the screen
//! snapshot and click-by-label (redaction, targeting), the inbox report, the diagnostic bundle, the grant-all-diagnostic button and the
//! extendable timer, the device nodes of the shell and the command-line texts. Everything runs against fakes.

use std::cell::RefCell;

use iyakki_sec::policy::Role;
use serde_json::{json, Value};

use super::api::{self, Ctx, Out};
use super::kube::{self, KubeOut, Kubectl};
use super::scopes::Scope;
use super::input::{self, InputEv};
use super::state::{self, Raw, RawField, Rect};
use super::tests_adb::{call, key, rig, Rig};
use super::*;

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

fn jv(o: &Out) -> Value {
    serde_json::from_slice(&o.body).unwrap_or(Value::Null)
}

struct FakeKube {
    seen: RefCell<Vec<Vec<String>>>,
    out: String,
    installed: bool,
}

impl FakeKube {
    fn new(out: &str) -> FakeKube {
        FakeKube { seen: RefCell::new(vec![]), out: out.into(), installed: true }
    }
}

impl Kubectl for FakeKube {
    fn installed(&self) -> bool {
        self.installed
    }
    fn run(&self, argv: &[String]) -> KubeOut {
        self.seen.borrow_mut().push(argv.to_vec());
        KubeOut { code: Some(0), stdout: self.out.clone(), ..Default::default() }
    }
}

fn call_kube(r: &Rig, k: &DevKey, kube: Option<&dyn Kubectl>, path: &str, a: &[&str]) -> Out {
    let policy = Role::Admin.template();
    let audit = |_: &str, _: &str| {};
    let tail = |n: usize| vec!["1 root login ok".to_string(); n.min(3)];
    let cx = Ctx { dm: &r.dm, key: k, policy: &policy, role: "admin", hostname: "iyakki-test", paths: &r.paths, audit_tail: &tail, audit: &audit, firewall_file: r.dir.join("firewall.json"), home: None, svc: None, ext: None, own_port: 7979, power: &|_| {}, boot: None, release: "", volatile: false, data_free: None, kube };
    let body = json!({ "args": a }).to_string();
    api::handle(&cx, "POST", path, &|_| String::new(), body.as_bytes()).unwrap()
}

// ---------------------------------------------------------------------------------------------- scopes and the timer

#[test]
fn the_kube_scope_is_off_by_default_needs_an_administrator_and_grant_all_diagnostic_never_gives_update_power_or_secrets() {
    let r = rig("diagscopes");
    let adm = Role::Admin.template();
    wr_keys(&r, &["install"]);
    assert!(!r.dm.keys()[0].has(Scope::Kube), "off by default");
    assert!(scopes::check(&key("l", "root", &[Scope::Install]), &adm, Scope::Kube).unwrap_err().contains("kube"));
    assert!(scopes::check(&key("l", "bob", &[Scope::Kube]), &Role::User.template(), Scope::Kube).unwrap_err().contains("administrator"));
    assert!(r.dm.grant_diagnostic(&Role::User.template(), "bob", "laptop").is_err(), "only an administrator grants");
    assert!(r.dm.grant_diagnostic(&adm, "root", "nobody").is_err());
    let k = r.dm.grant_diagnostic(&adm, "root", "laptop").unwrap();
    assert_eq!(k.scope_text(), "install, debug, files, input, shell, kube");
    for never in [Scope::Update, Scope::Power, Scope::Forward, Scope::FilesSystem, Scope::InputPassword, Scope::Config] {
        assert!(!k.has(never) || never == Scope::Power && k.has(Scope::Shell), "{} is not part of the diagnostic grant", never.id());
    }
    assert!(!k.scopes.iter().any(|s| ["update", "forward", "files:system", "input:password", "config"].contains(&s.as_str())));
    assert!(r.dm.activity(5).iter().any(|a| a.action == "devmode.scope.grant" && a.detail.contains("diagnostic")), "audited");
    // the grant is additive and repeatable
    assert_eq!(r.dm.grant_diagnostic(&adm, "root", "laptop").unwrap().scopes, k.scopes);
    // every scope has a one-line explanation
    for s in Scope::ALL {
        assert!(!s.describe().is_empty() && !s.describe().contains('\n'), "{}", s.id());
    }
}

fn wr_keys(r: &Rig, scopes: &[&str]) {
    let k = DevKey { name: "laptop".into(), key: "ab".repeat(32), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: scopes.iter().map(|s| s.to_string()).collect() };
    std::fs::create_dir_all(r.dm.dir()).unwrap();
    std::fs::write(r.dm.dir().join("keys.json"), serde_json::to_vec(&vec![k]).unwrap()).unwrap();
}

#[test]
fn the_switch_off_timer_can_be_extended_from_the_device_by_an_administrator_only() {
    let r = rig("extend");
    let adm = Role::Admin.template();
    // "until switched off" has nothing to extend
    assert!(r.dm.extend(&adm, "root", 4).unwrap_err().contains("no switch-off time"));
    r.dm.enable(&adm, "root", 4).unwrap();
    let before = r.dm.state().until;
    assert!(r.dm.extend(&Role::User.template(), "bob", 4).is_err());
    let st = r.dm.extend(&adm, "root", 4).unwrap();
    assert_eq!(st.until, before + 4 * 3600);
    assert!(r.dm.activity(3).iter().any(|a| a.action == "devmode.extend"));
    // never further than the longest allowed switch-off from now
    let st = r.dm.extend(&adm, "root", MAX_AUTO_OFF_HOURS).unwrap();
    assert!(st.until <= r.dm.now() + MAX_AUTO_OFF_HOURS as i64 * 3600);
    // off: refused
    r.dm.disable(&adm, "root").unwrap();
    assert!(r.dm.extend(&adm, "root", 4).is_err());
}

// ---------------------------------------------------------------------------------------------- kube

#[test]
fn read_only_verbs_pass_and_every_other_verb_is_refused_by_name() {
    for ok in ["get pods -A", "get nodes -o wide", "get pod web-1 -n default -o yaml", "describe pod web-1 -n kube-system", "logs web-1 -n default -c app --previous --tail 50", "top pods -A", "top nodes", "events -A", "events --for pod/web-1", "api-resources", "version", "auth can-i get pods", "get pods,svc -l app=web", "get deploy/web -n x -o json", "get configmaps -n x", "describe configmap cfg -n x", "get cm -o wide"] {
        assert!(kube::plan(&args(ok)).is_ok(), "{ok}: {:?}", kube::plan(&args(ok)));
    }
    for bad in ["exec -it web -- sh", "cp web:/etc/passwd .", "port-forward web 8080", "apply -f x.yaml", "delete pod web", "edit cm x", "patch pod web -p {}", "create ns x", "run x --image=y", "scale deploy web --replicas=0", "rollout restart deploy/web", "drain node1", "cordon node1", "label pod web a=b", "annotate pod web a=b", "taint nodes n a=b:NoSchedule", "set image deploy/web c=x", "replace -f x", "proxy", "attach web", "debug web", "config view", "wait pod web", "kustomize ."] {
        let e = kube::plan(&args(bad)).unwrap_err();
        assert!(e.contains("read-only"), "{bad}: {e}");
    }
    assert!(kube::plan(&args("frobnicate x")).unwrap_err().contains("unknown command"));
    assert!(kube::plan(&args("auth whoami")).unwrap_err().contains("can-i"));
    assert!(kube::plan(&[]).unwrap_err().contains("usage"));
    assert!(kube::plan(&args("--kubeconfig=/etc/rancher/k3s/k3s.yaml get pods")).unwrap_err().contains("verb first"));
}

#[test]
fn flags_that_pick_a_cluster_stream_read_files_or_go_raw_are_refused() {
    for bad in ["get pods --kubeconfig /etc/x", "get pods --server=https://evil", "get pods --token abc", "get pods --as admin", "get pods --context x", "get pods -w", "get pods --watch", "logs web -f", "logs web --follow", "get -f x.yaml", "get --raw /api", "get pods --insecure-skip-tls-verify", "get pods -- --x", "get pods --frobnicate", "get pods -o go-template-file=x", "get pods -o yaml-file", "get pods -o weird", "get pods -n 'a;b'", "get pods -l 'a$(x)'", "get pods -k dir", "get pods --show-managed-fields"] {
        assert!(kube::plan(&args(bad)).is_err(), "{bad} must be refused");
    }
    assert!(kube::plan(&args(&format!("get {}", "x".repeat(300)))).is_err(), "long argument");
    assert!(kube::plan(&["get".into(), "pods\nx".into()]).is_err(), "control character");
    assert!(kube::plan(&args(&format!("get pods {}", "a ".repeat(30)))).is_err(), "too many arguments");
}

#[test]
fn secrets_are_blocked_in_every_spelling_and_structured_configmaps_too() {
    for bad in ["get secrets", "get secret", "get secret/db-pass -n x", "get secrets -A -o yaml", "get pods,secrets", "get secrets.v1", "describe secret db", "get Secrets", "get sealedsecrets -A", "get pod/x secret/y", "events --for=secret/x", "logs secret/x"] {
        let e = kube::plan(&args(bad)).unwrap_err();
        assert!(e.to_lowercase().contains("secret"), "{bad}: {e}");
    }
    // a pod or a name that merely contains the word as a NAME is fine
    assert!(kube::plan(&args("get pods secret-keeper -n x")).is_ok());
    assert!(kube::plan(&args("get pod my-secret-pod")).is_ok());
    // ConfigMaps: plain output and describe work; structured output is blocked (one may hold a secret)
    assert!(kube::plan(&args("get cm -n x")).is_ok());
    for bad in ["get cm -n x -o yaml", "get configmaps -o json", "get configmap/x -o jsonpath={.data}", "get cm,pods -o go-template={{.}}", "get cm -o custom-columns=D:.data"] {
        assert!(kube::plan(&args(bad)).unwrap_err().contains("ConfigMap"), "{bad}");
    }
    // other kinds keep structured output
    assert!(kube::plan(&args("get pods -o yaml")).is_ok() && kube::plan(&args("get nodes -o json")).is_ok());
}

#[test]
fn logs_get_a_tail_and_a_byte_limit_and_the_plan_names_only_the_verb_and_the_kind() {
    let p = kube::plan(&args("logs web-1 -n default")).unwrap();
    assert!(p.argv.contains(&format!("--tail={}", kube::DEFAULT_TAIL)) && p.argv.iter().any(|a| a.starts_with("--limit-bytes=")), "{:?}", p.argv);
    let p = kube::plan(&args("logs web-1 --tail 7")).unwrap();
    assert!(p.argv.contains(&"--tail=7".to_string()) && !p.argv.iter().any(|a| a == &format!("--tail={}", kube::DEFAULT_TAIL)));
    assert!(kube::plan(&args("logs web-1 --tail 999999")).unwrap_err().contains("--tail"));
    assert!(kube::plan(&args("logs web-1 --tail -1")).is_err());
    let p = kube::plan(&args("get pods -n kube-system web-1")).unwrap();
    assert_eq!(p.audit(), "get pods", "no namespace, no name");
    assert_eq!(kube::plan(&args("top nodes")).unwrap().audit(), "top nodes");
    assert_eq!(kube::plan(&args("version")).unwrap().audit(), "version");
    assert_eq!(kube::plan(&args("get pods,svc,nodes")).unwrap().audit(), "get pods,svc,nodes");
    assert!(kube::plan(&args("top secrets")).is_err() && kube::plan(&args("top deployments")).is_err());
}

#[test]
fn the_kube_route_needs_the_scope_runs_the_fake_audits_verb_and_kind_and_caps_and_filters_the_answer() {
    let r = rig("kuberoute");
    let k = key("laptop", "root", &[Scope::Kube]);
    // without the scope: 403, naming it, and nothing ran
    let fake = FakeKube::new("x");
    let o = call_kube(&r, &key("laptop", "root", &[Scope::Debug]), Some(&fake), "/v1/dev/kube", &["get", "pods"]);
    assert_eq!(o.status, 403);
    assert!(String::from_utf8_lossy(&o.body).contains("kube") && fake.seen.borrow().is_empty());
    // with it
    let text = format!("NAME READY STATUS\nweb-1 1/1 Running\ntoken=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD\n{}", "pod-x 1/1 Running\n".repeat(10_000));
    let fake = FakeKube::new(&text);
    let o = call_kube(&r, &k, Some(&fake), "/v1/dev/kube", &["get", "pods", "-n", "kube-system", "secret-keeper"]);
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    let v = jv(&o);
    let t = v["text"].as_str().unwrap();
    assert!(t.contains("web-1 1/1 Running") && !t.contains("ghp_") && t.contains("hidden"), "a credential line is hidden");
    assert!(t.len() <= kube::MAX_OUT + 200 && v["truncated"] == true && t.contains("[... cut"), "capped: {} bytes", t.len());
    assert_eq!(fake.seen.borrow()[0][0], "get");
    // the log: verb and kind only
    let log: String = r.dm.activity(10).iter().map(|a| a.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("dev.kube get pods") && !log.contains("kube-system") && !log.contains("secret-keeper") && !log.contains("ghp_"), "{log}");
    // a refused command never reaches kubectl, and is logged as refused
    let fake = FakeKube::new("x");
    for bad in [vec!["delete", "pod", "web"], vec!["get", "secrets"], vec!["exec", "web", "--", "sh"], vec!["get", "pods", "-o", "yaml", "--kubeconfig=/x"]] {
        let o = call_kube(&r, &k, Some(&fake), "/v1/dev/kube", &bad);
        assert_eq!(o.status, 403, "{bad:?}");
    }
    assert!(fake.seen.borrow().is_empty());
    assert!(r.dm.activity(10).iter().any(|a| !a.ok && a.action == "dev.kube" && a.detail.contains("refused delete")));
    // no cluster installed / no kubectl
    let mut none = FakeKube::new("");
    none.installed = false;
    assert_eq!(call_kube(&r, &k, Some(&none), "/v1/dev/kube", &["get", "pods"]).status, 502);
    assert_eq!(call_kube(&r, &k, None, "/v1/dev/kube", &["get", "pods"]).status, 503);
    // junk body
    let o = call(&r, &k, &Role::Admin.template(), "POST", "/v1/dev/kube", &[], b"nope").unwrap();
    assert_eq!(o.status, 400);
}

// ---------------------------------------------------------------------------------------------- screen state

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn frame() -> Raw {
    Raw {
        app: "Settings".into(),
        page: "Network".into(),
        size: (1280, 720),
        texts: vec![
            (20.0, 20.0, "Cluster".into()),
            (20.0, 60.0, "API key".into()),
            (200.0, 60.0, "sk-live-abcdefghijklmnop".into()),
            (20.0, 100.0, "Password".into()),
            (200.0, 100.0, "••••••••".into()),
            (20.0, 140.0, "Server address".into()),
            (200.0, 140.0, "192.0.2.5".into()),
            (20.0, 200.0, "token=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD".into()),
            (20.0, 240.0, "   ".into()),
        ],
        hits: vec![("Use cluster address".into(), rect(400.0, 300.0, 200.0, 40.0)), ("Save".into(), rect(400.0, 400.0, 80.0, 40.0)), ("Reveal password".into(), rect(190.0, 95.0, 300.0, 30.0)), ("Cancel".into(), rect(500.0, 400.0, 80.0, 40.0))],
        fields: vec![RawField { rect: rect(190.0, 50.0, 300.0, 30.0), focused: false, secret: true }, RawField { rect: rect(190.0, 95.0, 300.0, 30.0), focused: false, secret: true }, RawField { rect: rect(190.0, 135.0, 300.0, 30.0), focused: true, secret: false }],
    }
}

#[test]
fn the_snapshot_hides_secret_fields_masks_and_credentials_names_the_focus_and_caps_its_lines() {
    let s = state::build(&frame());
    let all: String = s.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n");
    assert!(!all.contains("sk-live") && !all.contains("ghp_") && !all.contains("••"), "{all}");
    assert!(all.contains("Cluster") && all.contains("192.0.2.5") && all.contains("[hidden]"), "{all}");
    assert_eq!(s.lines.iter().filter(|l| l.text.trim().is_empty()).count(), 0, "blank texts are dropped");
    assert_eq!(s.app, "Settings");
    assert_eq!(s.page, "Network");
    assert_eq!(s.focused, "Server address", "the label beside the focused field");
    assert!(!s.focused_secret);
    // reading order
    assert!(s.lines.windows(2).all(|w| w[0].y <= w[1].y));
    // the focus on a password field is named without naming anything of it
    let mut f = frame();
    f.fields[1].focused = true;
    f.fields[2].focused = false;
    let s = state::build(&f);
    assert_eq!((s.focused.as_str(), s.focused_secret), ("a password field", true));
    // a button over a secret field has no name
    let s = state::build(&frame());
    let reveal = s.buttons.iter().find(|b| b.secret).expect("flagged");
    assert_eq!(reveal.label, "[secret field]");
    assert!(!s.text().contains("Reveal password"));
    // caps
    let mut big = frame();
    big.texts = (0..2000).map(|i| (10.0, i as f32 * 10.0, format!("line {i} {}", "x".repeat(500)))).collect();
    big.hits = (0..2000).map(|i| (format!("b{i}"), rect(0.0, i as f32, 10.0, 10.0))).collect();
    let s = state::build(&big);
    assert_eq!(s.lines.len(), state::MAX_TEXTS);
    assert_eq!(s.buttons.len(), state::MAX_HITS);
    assert!(s.lines.iter().all(|l| l.text.chars().count() <= state::MAX_LINE) && s.cut);
}

#[test]
fn private_apps_give_button_names_but_no_text() {
    for app in ["Chat", "Inbox", "CLI", "Browser", "Edit"] {
        let mut f = frame();
        f.app = app.into();
        f.texts.push((20.0, 300.0, "Dear Alice, the password is in the other message".into()));
        let s = state::build(&f);
        assert!(s.text_hidden && s.lines.is_empty(), "{app}");
        assert!(s.buttons.iter().any(|b| b.label == "Save"), "{app}: the buttons are still named");
        let t = s.text();
        assert!(!t.contains("Dear Alice") && t.contains("hidden (a private app"), "{t}");
        assert_eq!(s.focused, "a text field", "{app}: not even the label of the focused field");
    }
    let mut f = frame();
    f.app = "Settings".into();
    assert!(!state::build(&f).text_hidden);
}

#[test]
fn a_button_is_found_by_its_label_exactly_or_by_a_part_and_ambiguous_secret_or_covered_ones_are_refused() {
    let s = state::build(&frame());
    assert_eq!(s.target("Use cluster address").unwrap(), (500, 320, "Use cluster address".into()));
    assert_eq!(s.target("use CLUSTER address").unwrap().2, "Use cluster address", "any case");
    assert_eq!(s.target("cluster").unwrap().2, "Use cluster address", "one match by a part");
    assert!(s.target("nothing like it").unwrap_err().contains("no button"));
    assert!(s.target("").unwrap_err().contains("which button"));
    // "Cancel" and "Save" are different; "a" matches several: the error lists them
    let e = s.target("a").unwrap_err();
    assert!(e.contains("buttons match") && e.contains("Save"), "{e}");
    // the secret one
    assert!(s.target("secret field").unwrap_err().contains("secret"));
    // covered by a dialog drawn later at that place
    let mut f = frame();
    f.hits.push(("Dialog".into(), rect(380.0, 280.0, 300.0, 200.0)));
    let s = state::build(&f);
    assert!(s.target("Save").unwrap_err().contains("covered by \"Dialog\""));
    assert_eq!(s.target("Dialog").unwrap().2, "Dialog");
}

fn publish_frame(r: &Rig) {
    state::heartbeat(&r.dir);
    state::publish(&r.dir, &frame());
}

#[test]
fn state_and_click_routes_need_their_scopes_refuse_without_a_screen_and_click_queues_a_tap_by_label() {
    let r = rig("stateroutes");
    let a = Role::Admin.template();
    let dbg = key("laptop", "root", &[Scope::Debug]);
    let inp = key("laptop", "root", &[Scope::Input]);
    // no screen is drawing
    assert_eq!(call(&r, &dbg, &a, "GET", "/v1/dev/state", &[], b"").unwrap().status, 503);
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"Save"}"#).unwrap().status, 503);
    publish_frame(&r);
    // scopes
    assert_eq!(call(&r, &inp, &a, "GET", "/v1/dev/state", &[], b"").unwrap().status, 403);
    assert_eq!(call(&r, &dbg, &a, "POST", "/v1/dev/click", &[], br#"{"label":"Save"}"#).unwrap().status, 403);
    let o = call(&r, &dbg, &a, "GET", "/v1/dev/state", &[], b"").unwrap();
    assert_eq!(o.status, 200);
    let v = jv(&o);
    assert_eq!(v["app"], "Settings");
    assert!(v["text"].as_str().unwrap().contains("Use cluster address") && !v["text"].as_str().unwrap().contains("sk-live"));
    // click: a tap at the centre of the button
    let o = call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"Use cluster address"}"#).unwrap();
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    assert_eq!(input::take(&r.dir), vec![InputEv::Tap(500, 320)]);
    // unknown, secret and junk
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"nope"}"#).unwrap().status, 404);
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"Reveal password"}"#).unwrap().status, 404, "its name is hidden: it cannot even be asked for by name");
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"secret field"}"#).unwrap().status, 403);
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":""}"#).unwrap().status, 422);
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], b"x").unwrap().status, 400);
    assert!(input::take(&r.dir).is_empty(), "nothing else was queued");
    // a password field has focus: refused like every other input
    input::set_secret_focus(&r.dir, true);
    assert_eq!(call(&r, &inp, &a, "POST", "/v1/dev/click", &[], br#"{"label":"Save"}"#).unwrap().status, 403);
    input::set_secret_focus(&r.dir, false);
    // the log has counts and the label, never the screen's text
    let log: String = r.dm.activity(40).iter().map(|x| x.line()).collect::<Vec<_>>().join("\n");
    assert!(log.contains("click \"Use cluster address\"") && log.contains("Settings:") && !log.contains("192.0.2.5") && !log.contains("sk-live"), "{log}");
    state::clear(&r.dir);
}

// ---------------------------------------------------------------------------------------------- inbox

fn note(i: usize, kind: crate::inbox::Kind, source: &str, title: &str, body: &str, read: bool) -> crate::inbox::Notification {
    serde_json::from_value(json!({ "id": format!("{i:02x}"), "time": 1_800_000_000 + i as i64, "kind": kind.name(), "title": title, "body": body, "source": source, "read": read })).expect("a notification")
}

#[test]
fn the_inbox_report_counts_by_kind_and_source_lists_titles_and_gives_bodies_only_when_asked_and_scrubbed() {
    use crate::inbox::Kind;
    let items = vec![
        note(1, Kind::Error, "k3s", "Cluster stopped", "node lost: token=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD", false),
        note(2, Kind::Info, "download", "Download finished", "the private body", true),
        note(3, Kind::Error, "k3s", "Cluster stopped again", "x", false),
    ];
    let v = super::api_diag::inbox_report(&items, false);
    assert_eq!((v["total"].as_u64(), v["unread"].as_u64()), (Some(3), Some(2)));
    let kinds: Vec<(String, u64)> = v["by_kind"].as_array().unwrap().iter().map(|x| (x["kind"].as_str().unwrap().to_string(), x["count"].as_u64().unwrap())).collect();
    assert!(kinds.contains(&("error".to_string(), 2)) && kinds.contains(&("info".to_string(), 1)), "{kinds:?}");
    assert_eq!(v["by_source"][0]["source"], "k3s");
    assert_eq!(v["items"][0]["title"], "Cluster stopped");
    let text = v.to_string();
    assert!(!text.contains("the private body") && !text.contains("node lost") && v["items"][0].get("body").is_none(), "no bodies by default");
    let v = super::api_diag::inbox_report(&items, true);
    let text = v.to_string();
    assert!(text.contains("the private body") && !text.contains("ghp_") && text.contains("hidden"), "bodies are scrubbed: {text}");
    // titles are capped
    let long = vec![note(1, Kind::Info, "", &"t".repeat(500), "", false)];
    assert_eq!(super::api_diag::inbox_report(&long, false)["items"][0]["title"].as_str().unwrap().len(), super::api_diag::MAX_TITLE);
    assert_eq!(super::api_diag::inbox_report(&long, false)["by_source"][0]["source"], "(none)");
}

#[test]
fn inbox_bodies_need_the_files_system_scope_as_well_as_debug() {
    let r = rig("inbox");
    let a = Role::Admin.template();
    let dbg = key("laptop", "root", &[Scope::Debug]);
    let o = call(&r, &dbg, &a, "GET", "/v1/dev/inbox", &[("bodies", "1")], b"").unwrap();
    assert_eq!(o.status, 403);
    assert!(String::from_utf8_lossy(&o.body).contains("files:system"));
    // titles only: allowed with debug (503 here only because the test device has no services)
    assert_eq!(call(&r, &dbg, &a, "GET", "/v1/dev/inbox", &[], b"").unwrap().status, 503);
    let both = key("laptop", "root", &[Scope::Debug, Scope::Files, Scope::FilesSystem]);
    assert_eq!(call(&r, &both, &a, "GET", "/v1/dev/inbox", &[("bodies", "1")], b"").unwrap().status, 503, "past the scope check");
    assert_eq!(call(&r, &key("l", "root", &[Scope::Install]), &a, "GET", "/v1/dev/inbox", &[], b"").unwrap().status, 403);
}

// ---------------------------------------------------------------------------------------------- diag

#[test]
fn the_diagnostic_bundle_has_what_a_helper_needs_and_no_secrets() {
    let r = rig("diag");
    // secrets where a careless bundle would find them: the kernel command line, the logs, the boot log, the audit log, the cluster
    std::fs::write(r.paths.sys.proc.join("cmdline"), "quiet root=/dev/sda2 iyakki.token=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD\n").unwrap();
    std::fs::write(&r.paths.boot_log, "boot ok\nerror: mount failed\nerror: login password=hunter2hunter2\nnormal line\n").unwrap();
    let a = Role::Admin.template();
    let k = key("laptop", "root", &[Scope::Debug, Scope::Kube]);
    let fake = FakeKube::new("NAME STATUS ROLES AGE VERSION\nmac Ready control-plane 3d v1.30\n");
    let policy = a;
    let audit = |_: &str, _: &str| {};
    let tail = |_: usize| vec!["1 root login ok".to_string(), "2 root token=ghp_abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_string()];
    let cx = Ctx { dm: &r.dm, key: &k, policy: &policy, role: "admin", hostname: "iyakki-test", paths: &r.paths, audit_tail: &tail, audit: &audit, firewall_file: r.dir.join("firewall.json"), home: None, svc: None, ext: None, own_port: 7979, power: &|_| {}, boot: None, release: "", volatile: false, data_free: None, kube: Some(&fake) };
    state::heartbeat(&r.dir);
    state::publish(&r.dir, &frame());
    crate::devname::write_status(&r.dir, &crate::devname::Status { on: true, host: "iyakki-0a1b2c.local".into(), probing: false, addresses: vec!["192.0.2.10".into()], services: vec![], note: String::new() });
    let o = api::handle(&cx, "GET", "/v1/dev/diag", &|_| String::new(), b"").unwrap();
    assert_eq!(o.status, 200);
    let t = jv(&o)["text"].as_str().unwrap().to_string();
    for must in ["0.1.0-alpha.1", "macweb", "MacBookPro14,1", "uptime 93784", "192.0.2.10", "HDA-Intel", "brcm/brcmfmac43602-pcie.bin", "mac  Ready", "open app: Settings / Network", "shell started", "login ok", "boot: error: mount failed", "dev cluster", "services", "device name (mDNS): iyakki-0a1b2c.local at 192.0.2.10"] {
        assert!(t.contains(must), "missing {must:?} in:\n{t}");
    }
    for secret in ["ghp_", "sk-live", "hunter2", "iyakki.token", "root=/dev/sda2", "192.0.2.5", "API key"] {
        assert!(!t.contains(secret), "the bundle must not hold {secret:?}:\n{t}");
    }
    assert!(t.len() <= diag::MAX_BUNDLE + 16);
    // only names and status of the cluster: the fake got two plain `get`s
    let seen = fake.seen.borrow();
    assert!(seen.iter().all(|a| a[0] == "get" && !a.iter().any(|x| x == "-o")), "{seen:?}");
    drop(seen);
    // debug alone: no cluster part, and a user account gets no logs
    let dbg = key("laptop", "bob", &[Scope::Debug]);
    let user = Role::User.template();
    let cx = Ctx { key: &dbg, policy: &user, kube: Some(&fake), ..cx };
    let t = jv(&api::handle(&cx, "GET", "/v1/dev/diag", &|_| String::new(), b"").unwrap())["text"].as_str().unwrap().to_string();
    assert!(t.contains("does not hold the kube scope") && t.contains("administrator accounts") && !t.contains("login ok"), "{t}");
    state::clear(&r.dir);
}

// ---------------------------------------------------------------------------------------------- the shell's device nodes and environment

#[test]
fn the_shell_makes_sure_dev_null_and_friends_can_be_opened_by_an_ordinary_user() {
    use super::shellexec::{command_env, ensure_std_nodes, node_fix, NodeFix, STD_NODES};
    // missing, a plain file, wrong numbers: made again; a root-only device: opened up; a right one: left alone
    assert_eq!(node_fix((1, 3), None), vec![NodeFix::Recreate]);
    assert_eq!(node_fix((1, 3), Some((false, 0o644, 0, 0))), vec![NodeFix::Recreate]);
    assert_eq!(node_fix((1, 3), Some((true, 0o666, 1, 5))), vec![NodeFix::Recreate]);
    assert_eq!(node_fix((1, 3), Some((true, 0o600, 1, 3))), vec![NodeFix::Chmod(0o666)], "the case of the Mac: root-only /dev/null");
    assert_eq!(node_fix((1, 3), Some((true, 0o660, 1, 3))), vec![NodeFix::Chmod(0o666)]);
    assert!(node_fix((1, 3), Some((true, 0o666, 1, 3))).is_empty());
    assert!(STD_NODES.iter().any(|(n, a, b)| *n == "null" && (*a, *b) == (1, 3)) && STD_NODES.iter().any(|(n, _, _)| *n == "urandom"));
    // against a folder that has none of them: every node is tried and reported; a process that may not make devices says so
    let dir = std::env::temp_dir().join(format!("iyakki-nodes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let notes = ensure_std_nodes(&dir);
    assert_eq!(notes.len(), STD_NODES.len(), "{notes:?}");
    assert!(notes.iter().all(|n| n.contains("made again") || n.contains("could not be fixed")));
    let _ = std::fs::remove_dir_all(&dir);
    // the environment of a command: clean, with the account's own home and name
    let env = command_env(Path::new("/home/admin"), "admin");
    let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
    assert_eq!(get("HOME").as_deref(), Some("/home/admin"));
    assert_eq!(get("USER").as_deref(), Some("admin"));
    assert_eq!(get("IYAKKI_DEV").as_deref(), Some("1"));
    assert!(get("PATH").unwrap().contains("/usr/bin") && get("PATH").unwrap().contains("/sbin"));
    assert!(env.iter().all(|(n, _)| !["IYAKKI_TOKEN", "HISTFILE", "SSH_AUTH_SOCK"].contains(n)), "nothing is inherited");
}

// ---------------------------------------------------------------------------------------------- the command-line texts

#[test]
fn the_usage_text_lists_the_diagnosis_commands_whoami_explains_every_scope_and_a_kube_refusal_needs_no_network() {
    let u = cli::usage();
    for w in ["kube get|describe|logs|top|events", "state [--json]", "input click", "inbox [--bodies]", "diag [-o FILE]", "swipe X1 Y1 X2 Y2 | click"] {
        assert!(u.contains(w), "{w} in the usage text");
    }
    assert!(!u.contains("{DIAG}"));
    let lines = cli_diag::scope_lines(&["install".into(), "debug".into(), "shell".into()]);
    assert_eq!(lines.len(), 1 + Scope::ALL.len());
    for s in Scope::ALL {
        let l = lines.iter().find(|l| l.contains(&format!(" {} ", s.id())) || l.contains(&format!(" {:<15} ", s.id()))).unwrap_or_else(|| panic!("a line for {}", s.id()));
        assert!(l.contains(s.describe()), "{l}");
    }
    assert!(lines.iter().any(|l| l.starts_with("  [x] debug")) && lines.iter().any(|l| l.starts_with("  [ ] kube")) && lines.iter().any(|l| l.starts_with("  [x] power")), "shell includes power: {lines:?}");
    // raw words: the CLI's own options are not given to kubectl
    assert_eq!(cli_diag::kube_args(&args("--device mac kube get pods -n x --home /h")), args("get pods -n x"));
    assert_eq!(cli_diag::kube_args(&args("kube get pods --device=mac")), args("get pods"));
    // a refused command fails before any network is touched
    let home = std::env::temp_dir().join(format!("iyakki-clihome-{}", std::process::id()));
    let mut printed = Vec::new();
    for bad in ["kube delete pod x", "kube get secrets", "kube exec x -- sh"] {
        let mut a = args(bad);
        a.extend(["--home".to_string(), home.display().to_string()]);
        let e = cli::run(&a, &mut |l| printed.push(l.to_string())).unwrap_err();
        assert!(e.contains("read-only") || e.contains("Secrets"), "{bad}: {e}");
    }
    let mut out = String::new();
    cli::run(&args("kube help"), &mut |l| out.push_str(l)).unwrap();
    assert!(out.contains("Secrets are blocked"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn every_new_route_is_in_the_table_with_its_scope() {
    assert_eq!(api::scope_of("POST", "/v1/dev/kube"), Some(Some(Scope::Kube)));
    assert_eq!(api::scope_of("GET", "/v1/dev/state"), Some(Some(Scope::Debug)));
    assert_eq!(api::scope_of("GET", "/v1/dev/inbox"), Some(Some(Scope::Debug)));
    assert_eq!(api::scope_of("GET", "/v1/dev/diag"), Some(Some(Scope::Debug)));
    assert_eq!(api::scope_of("POST", "/v1/dev/click"), Some(Some(Scope::Input)));
    assert_eq!(Scope::parse("kube"), Some(Scope::Kube));
}
