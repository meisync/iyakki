//! "Credentials always live in the vault": what must never hold a secret value, checked by planting one and looking.

use std::sync::{Arc, Mutex};

use super::connector::fake::*;
use super::connector::*;
use super::model::*;
use super::registry;
use super::store::{Dashboards, Sources};
use crate::wsexec::FakeRunner;
use serde_json::json;

const SECRET: &str = "Zq9-planted-SECRET-4417";

fn info(kind: &str) -> KindInfo {
    registry::kind(kind).unwrap()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-dash-sec-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn addresses_with_a_user_a_password_or_a_token_are_refused_everywhere() {
    let i = info("prometheus");
    for (url, why) in [
        ("https://user:pass@prom.example:9090", "do not put a user name"),
        ("https://user@prom.example", "do not put a user name"),
        ("https://prom.example/api?api_key=abc123", "api_key="),
        ("https://prom.example/?foo=1&token=abc", "token="),
        ("https://prom.example/?access_token=abc", "access_token="),
        ("https://prom.example/?Password=abc", "password="),
        ("https://prom.example/?x-api-key=abc", "x-api-key="),
        ("ftp://prom.example", "must start with http"),
    ] {
        let mut c = cfg("prometheus", url);
        c.name = "p".into();
        let p = validate(&c, &i).join(" | ");
        assert!(p.contains(why), "{url}: {p}");
        assert!(Sources::open(&tmp("url"), "amy").save(c).is_err(), "{url} is not saved");
    }
    // the proxy too
    let mut c = cfg("prometheus", "https://prom.example");
    c.name = "p".into();
    c.values.insert("proxy".into(), "http://bob:hunter2@proxy:3128".into());
    assert!(validate(&c, &i).join(" ").contains("do not put a user name"));
    // a harmless query string is fine
    let mut ok = cfg("prometheus", "https://prom.example/prometheus?org=1");
    ok.name = "p".into();
    assert!(validate(&ok, &i).is_empty(), "{:?}", validate(&ok, &i));
}

#[test]
fn a_secret_cannot_hide_in_a_value_under_any_name_or_look_like_a_setting() {
    let i = info("elasticsearch");
    let mut c = cfg("elasticsearch", "https://es.example:9200");
    c.name = "es".into();
    c.values.insert("password".into(), SECRET.into());
    c.values.insert("authorization".into(), format!("Bearer {SECRET}"));
    c.values.insert("default_index".into(), "password: hunter2hunter2".into());
    let p = validate(&c, &i).join(" | ");
    assert!(p.contains("never stored in the settings") && p.contains("unknown setting `authorization`") && p.contains("looks like a password or a token"), "{p}");
    assert!(!p.contains(SECRET), "a rejected value is not echoed: {p}");
}

#[test]
fn an_imported_source_loses_its_plaintext_and_says_so() {
    let i = info("prometheus");
    let mut c = cfg("prometheus", &format!("https://user:{SECRET}@prom.example/api?token={SECRET}&org=3"));
    c.name = "Prod".into();
    c.values.insert("password".into(), SECRET.into());
    c.values.insert("auth".into(), "basic".into());
    c.values.insert("sneaky_header".into(), SECRET.into());
    c.secrets.insert("password".into(), SECRET.into());
    c.secrets.insert("token".into(), "uvault:svc.token".into());
    let (clean, notes) = sanitize_imported_source(c, &i);
    let all = format!("{clean:?} {notes:?}");
    assert!(!all.contains(SECRET), "{all}");
    assert_eq!(clean.value("url"), "https://prom.example/api?org=3");
    assert!(!clean.secrets.contains_key("password") && clean.secrets["token"] == "uvault:svc.token");
    assert!(notes.iter().any(|n| n.contains("plain-text Password was removed")) && notes.iter().any(|n| n.contains("was not a vault reference")) && notes.iter().any(|n| n.contains("user name and password in the address")), "{notes:?}");
    assert!(validate(&clean, &i).is_empty(), "the cleaned source is valid: {:?}", validate(&clean, &i));
}

#[test]
fn an_export_never_holds_a_secret_and_an_import_maps_the_vault_names() {
    let dir = tmp("export");
    let dashes = Dashboards::open(&dir);
    let mut src = cfg("prometheus", &format!("https://u:{SECRET}@prom.example?api_key={SECRET}"));
    src.id = "prom".into();
    src.name = "Prod".into();
    src.values.insert("auth".into(), "bearer".into());
    src.values.insert("token".into(), SECRET.into());
    src.secrets.insert("token".into(), "uvault:svc.prom".into());
    let mut d = Dashboard::new("d", "Board");
    let mut p = Panel::new("", "CPU", "prom", Visual::Line, &format!("up # password: {SECRET}{SECRET}"), Grid::new(0, 0, 12, 6));
    p.expressions.push(format!("C = A * 2 # api key {SECRET}"));
    d.add_panel(p).unwrap();
    let bundle = Dashboards::export_bundle(&d, &[src.clone()]);
    let plain = Dashboards::export_text(&d);
    for text in [&bundle, &plain] {
        assert!(!text.contains(SECRET), "a planted secret leaked into an export:\n{text}");
    }
    assert!(bundle.contains("uvault:svc.prom"), "the placeholder name is there");
    // saving a dashboard scrubs too
    dashes.save(&d).unwrap();
    assert!(!std::fs::read_to_string(dashes.dir().join("d.json")).unwrap().contains(SECRET));
    // import: the importer has an entry of that name -> kept; one without -> dropped and listed
    let with = dashes.import_bundle(&bundle, &["svc.prom".to_string()]).unwrap();
    assert_eq!(with.sources.len(), 1);
    assert_eq!(with.sources[0].secrets["token"], "uvault:svc.prom");
    assert!(with.needs_entry.is_empty());
    let without = dashes.import_bundle(&bundle, &["svc.other".to_string()]).unwrap();
    assert!(!without.sources[0].secrets.contains_key("token"));
    assert_eq!(without.needs_entry, vec![("prom".to_string(), "Token / API key".to_string())]);
    assert!(without.warnings.iter().any(|w| w.contains("svc.prom") && w.contains("not in your vault")), "{:?}", without.warnings);
    // a bundle with plaintext planted by hand is cleaned on the way in
    let mut v: serde_json::Value = serde_json::from_str(&bundle).unwrap();
    v["sources"][0]["values"]["password"] = json!(SECRET);
    v["sources"][0]["secrets"]["token"] = json!(SECRET);
    v["dashboard"]["panels"][0]["query"]["text"] = json!(format!("up password: {SECRET}{SECRET}"));
    let evil = dashes.import_bundle(&v.to_string(), &[]).unwrap();
    assert!(!format!("{:?}", (&evil.sources, &evil.dashboard)).contains(SECRET), "{:?}", evil.warnings);
    assert!(!evil.warnings.join(" ").contains(SECRET));
    // a plain dashboard file imports as before
    assert!(dashes.import_bundle(&plain, &[]).unwrap().sources.is_empty());
    assert!(dashes.import_bundle("{nope", &[]).is_err());
}

/// A server that sends the credential back: in an error, in the body, in a transport failure.
struct Echo {
    mode: u8,
    seen: Mutex<Vec<String>>,
}

impl iyakki_proxy::Upstream for Echo {
    fn request(&self, _m: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        self.seen.lock().unwrap().push(format!("{url} {}", String::from_utf8_lossy(body)));
        let auth = headers.iter().find(|h| h.0 == "Authorization").map(|h| h.1.clone()).unwrap_or_default();
        match self.mode {
            0 => Ok(iyakki_proxy::UpstreamResponse { status: 400, body: format!(r#"{{"status":"error","errorType":"bad_data","error":"bad request, headers were: {auth}"}}"#).into_bytes() }),
            1 => Ok(iyakki_proxy::UpstreamResponse { status: 200, body: format!(r#"{{"status":"success","data":{{"resultType":"vector","result":[{{"metric":{{"echo":"{auth}","raw":"{}"}},"value":[1792069200,"1"]}}]}}}}"#, auth.trim_start_matches("Bearer ")).into_bytes() }),
            _ => Err(format!("connection refused while sending {auth}")),
        }
    }
}

struct EchoTransport(Arc<Echo>);
impl Transport for EchoTransport {
    fn upstream(&self, _: &NetOptions) -> Result<Arc<dyn iyakki_proxy::Upstream>, String> {
        Ok(self.0.clone())
    }
}

fn bearer_source() -> SourceConfig {
    let mut c = cfg("prometheus", "https://prom.example");
    c.values.insert("auth".into(), "bearer".into());
    c.secrets.insert("token".into(), "uvault:svc.prom".into());
    c
}

fn echo_deps(e: &Arc<Echo>) -> Deps {
    let d = deps(&FakeUp::new(), &[("uvault:svc.prom", SECRET)], Arc::new(FakeRunner::new(&[])));
    Deps { transport: Arc::new(EchoTransport(e.clone())), ..d }
}

#[test]
fn a_server_that_echoes_the_credential_cannot_leak_it_into_frames_errors_or_text() {
    for mode in 0..3u8 {
        let e = Arc::new(Echo { mode, seen: Mutex::new(vec![]) });
        let s = registry::build(&bearer_source(), &echo_deps(&e)).unwrap();
        let mut r = QueryRequest::new("up", range());
        r.step_ms = None;
        let out = s.query(&r, &Cancel::new());
        let shown = match &out {
            Ok(f) => format!("{f:?} {} {} {}", f.markdown(50, 5000), f.csv(), f.summary()),
            Err(er) => format!("{er:?} {er}"),
        };
        assert!(!shown.contains(SECRET), "mode {mode}: {shown}");
        if mode != 1 {
            assert!(shown.contains("redacted") || out.is_err(), "mode {mode}: {shown}");
        }
        // the credential was in no URL or body
        assert!(e.seen.lock().unwrap().iter().all(|l| !l.contains(SECRET)));
        let t = s.test(&Cancel::new());
        assert!(!format!("{t:?}").contains(SECRET));
    }
}

#[test]
fn a_locked_vault_and_a_missing_entry_have_their_own_plain_messages() {
    struct R(&'static str);
    impl SecretResolver for R {
        fn resolve(&self, _: &str) -> Result<Zeroizing<String>, String> {
            Err(self.0.to_string())
        }
    }
    let up = FakeUp::new();
    for (err, want, not) in [("vault is locked", "your vault is locked", "missing"), ("secret not found: svc.prom", "is not in your vault yet (entry `svc.prom`)", "locked"), ("backend exploded", "could not be read", "locked")] {
        let mut d = deps(&up, &[], Arc::new(FakeRunner::new(&[])));
        d.secrets = Arc::new(R(err));
        let e = registry::build(&bearer_source(), &d).err().unwrap();
        assert!(e.message.contains(want) && !e.message.contains(not), "{err}: {e}");
        assert!(e.fix.is_some());
        assert_eq!(e.kind, ErrKind::Config);
    }
}

#[test]
fn old_panels_load_as_query_a_and_new_ones_round_trip() {
    let old = r#"{"id":"x","title":"Old","panels":[{"id":"p1","title":"CPU","source":"prom","query":{"text":"up","target":null},"visual":"line","grid":{"x":0,"y":0,"w":12,"h":8}}]}"#;
    let d: Dashboard = serde_json::from_str(old).unwrap();
    let defs = d.panels[0].query_defs("fallback");
    assert_eq!(defs.len(), 1);
    assert_eq!((defs[0].0, defs[0].1.source.as_str(), defs[0].1.query.text.as_str()), ('A', "prom", "up"));
    assert!(d.panels[0].queries.is_empty() && d.panels[0].expressions.is_empty() && d.default_source.is_empty());
    // a panel with no source takes the dashboard's default
    let mut p = d.panels[0].clone();
    p.source = String::new();
    p.queries.push(QueryDef { source: "es".into(), query: PanelQuery { text: "{}".into(), axis: Axis::Right, ..Default::default() } });
    let defs = p.query_defs("prom");
    assert_eq!((defs[0].1.source.as_str(), defs[1].0, defs[1].1.source.as_str()), ("prom", 'B', "es"));
    assert_eq!(p.sources("prom"), vec!["prom".to_string(), "es".to_string()]);
    let back: Panel = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
}
