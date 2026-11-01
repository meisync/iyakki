use crate::utcp::*;
use iyakki_proxy::{Upstream, UpstreamResponse};
use serde_json::json;
use std::sync::Mutex;

/// Records the request and answers with a fixed status and body.
struct Fake {
    seen: Mutex<Vec<(String, String, Vec<(String, String)>, Vec<u8>)>>,
    status: u16,
    body: Vec<u8>,
}

impl Fake {
    fn new(status: u16, body: &str) -> Fake {
        Fake { seen: Mutex::new(vec![]), status, body: body.as_bytes().to_vec() }
    }
}

impl Upstream for Fake {
    fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((m.into(), url.into(), h.to_vec(), b.to_vec()));
        Ok(UpstreamResponse { status: self.status, body: self.body.clone() })
    }
}

fn no_secrets(_: &str) -> Option<String> {
    None
}

const MANUAL: &str = r#"{
  "utcp_version": "1.0.1", "manual_version": "2.0.0",
  "info": {"title": "Weather", "version": "2.0.0", "description": "Forecasts"},
  "tools": [
    {"name": "get_forecast", "description": "Forecast for a city",
     "inputs": {"type": "object", "properties": {"city": {"type": "string"}, "days": {"type": "integer"}}, "required": ["city"]},
     "outputs": {"type": "object"}, "tags": ["weather"],
     "tool_call_template": {"call_template_type": "http", "url": "https://api.example.com/v1/{city}/forecast", "http_method": "GET"}},
    {"name": "post_note", "description": "Save a note",
     "inputs": {"type": "object", "properties": {"note": {"type": "object"}, "X-Trace": {"type": "string"}}},
     "tool_call_template": {"call_template_type": "http", "url": "https://api.example.com/notes", "http_method": "post",
        "body_field": "note", "header_fields": ["X-Trace"],
        "auth": {"auth_type": "api_key", "api_key": "Bearer ${WEATHER_KEY}", "var_name": "Authorization", "location": "header"}}},
    {"name": "run_local", "description": "A command",
     "tool_call_template": {"call_template_type": "cli", "commands": [{"command": "ls"}]}}
  ]
}"#;

#[test]
fn a_manual_is_read_and_what_cannot_be_used_is_listed_with_the_reason() {
    let m = parse_manual(MANUAL, false).unwrap();
    assert_eq!((m.utcp_version.as_str(), m.manual_version.as_str(), m.title.as_str()), ("1.0.1", "2.0.0", "Weather"));
    assert_eq!(m.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["get_forecast", "post_note"]);
    assert_eq!(m.tools[1].template.method, "POST", "the method is upper-cased");
    assert_eq!(m.skipped.len(), 1, "{:?}", m.skipped);
    assert!(m.skipped[0].starts_with("run_local") && m.skipped[0].contains("allowed_communication_protocols"), "{:?}", m.skipped);
}

#[test]
fn a_manual_must_say_what_it_is() {
    assert!(parse_manual("not json", false).unwrap_err().contains("not JSON"));
    assert!(parse_manual(r#"{"openapi":"3.0.0","paths":{}}"#, false).unwrap_err().contains("utcp_version"));
    assert!(parse_manual(r#"{"utcp_version":"1.0.1"}"#, false).unwrap_err().contains("tools"));
    let big = format!(r#"{{"utcp_version":"1","tools":[],"pad":"{}"}}"#, "x".repeat(MAX_MANUAL));
    assert!(parse_manual(&big, false).unwrap_err().contains("bigger"));
}

#[test]
fn the_cli_template_is_allowed_by_the_manual_but_still_not_supported_yet() {
    let t = r#"{"utcp_version":"1","allowed_communication_protocols":["http","cli"],"tools":[{"name":"a","tool_call_template":{"call_template_type":"cli","commands":[{"command":"ls"}]}}]}"#;
    let m = parse_manual(t, true).unwrap();
    assert!(m.tools.is_empty() && m.skipped[0].contains("not supported yet"), "{:?}", m.skipped);
}

#[test]
fn only_https_and_only_this_machine_may_use_plain_http() {
    assert!(check_tool_url("https://api.example.com/x", false).is_ok());
    assert!(check_tool_url("http://api.example.com/x", false).unwrap_err().contains("not https"));
    assert!(check_tool_url("http://127.0.0.1:8080/x", true).is_ok());
    assert!(check_tool_url("http://localhost/x", true).is_ok());
    assert!(check_tool_url("ftp://example.com/x", false).is_err());
    assert!(check_tool_url("https://user:pw@example.com/x", false).is_err(), "no user names in the address");
}

#[test]
fn a_manual_from_the_internet_may_not_point_at_this_machine_or_its_network() {
    for u in ["https://localhost/x", "https://127.0.0.1/x", "https://10.0.0.5/x", "https://192.168.1.2/x", "https://172.16.3.4/x", "https://169.254.1.1/x", "https://[::1]/x", "https://[fd00::1]/x", "https://printer.local/x", "https://intranet/x"] {
        assert!(check_tool_url(u, false).unwrap_err().contains("this machine or its network"), "{u}");
        assert!(check_tool_url(u, true).is_ok(), "a local manual may use {u}");
    }
    let bad = MANUAL.replace("https://api.example.com/notes", "https://192.168.1.9/notes");
    let m = parse_manual(&bad, false).unwrap();
    assert!(m.skipped.iter().any(|s| s.starts_with("post_note") && s.contains("network")), "{:?}", m.skipped);
    assert!(parse_manual(&bad, true).unwrap().skipped.len() == 1, "from a local manual the address is fine");
}

#[test]
fn path_values_query_values_and_headers_go_where_the_template_says_and_are_encoded() {
    let m = parse_manual(MANUAL, false).unwrap();
    let p = prepare(&m.tools[0], &json!({"city": "San José/x?y", "days": 3}), &no_secrets).unwrap();
    assert_eq!(p.method, "GET");
    assert_eq!(p.url, "https://api.example.com/v1/San%20Jos%C3%A9%2Fx%3Fy/forecast?days=3", "a path value cannot add a / or a ?");
    assert!(p.body.is_empty());
    assert!(prepare(&m.tools[0], &json!({"days": 3}), &no_secrets).unwrap_err().contains("`city` is required"));
    assert!(prepare(&m.tools[0], &json!("x"), &no_secrets).unwrap_err().contains("JSON object"));
}

#[test]
fn the_body_field_the_header_fields_and_the_vault_key_are_used_and_the_key_is_never_in_the_manual() {
    let m = parse_manual(MANUAL, false).unwrap();
    let vault = |n: &str| (n == "WEATHER_KEY").then(|| "sekret-123".to_string());
    let p = prepare(&m.tools[1], &json!({"note": {"text": "hi"}, "X-Trace": "abc", "extra": "1"}), &vault).unwrap();
    assert_eq!(p.method, "POST");
    assert_eq!(p.url, "https://api.example.com/notes?extra=1", "what is left over goes to the query");
    assert_eq!(p.body, br#"{"text":"hi"}"#);
    assert!(p.headers.contains(&("Authorization".into(), "Bearer sekret-123".into())));
    assert!(p.headers.contains(&("X-Trace".into(), "abc".into())));
    assert!(p.headers.contains(&("Content-Type".into(), "application/json".into())));
    assert!(!MANUAL.contains("sekret-123"));
    assert!(prepare(&m.tools[1], &json!({"note": {}}), &no_secrets).unwrap_err().contains("no secret called WEATHER_KEY"));
    let literal = MANUAL.replace("Bearer ${WEATHER_KEY}", "Bearer abc123");
    assert!(parse_manual(&literal, false).unwrap().skipped.iter().any(|s| s.contains("keep the key in the vault")));
    assert!(parse_manual(&MANUAL.replace("\"api_key\", \"api_key\"", "\"oauth2\", \"x\""), false).is_ok());
}

#[test]
fn a_header_value_with_a_line_break_is_refused() {
    let m = parse_manual(MANUAL, false).unwrap();
    let vault = |_: &str| Some("k".to_string());
    let e = prepare(&m.tools[1], &json!({"note": {}, "X-Trace": "a\r\nHost: evil"}), &vault).unwrap_err();
    assert!(e.contains("line break"), "{e}");
}

#[test]
fn a_call_sends_the_request_and_returns_the_reply_and_errors_say_the_status() {
    let m = parse_manual(MANUAL, false).unwrap();
    let ok = Fake::new(200, r#"{"temp": 21}"#);
    assert_eq!(call(&ok, &m, "get_forecast", &json!({"city": "Oslo"}), &no_secrets).unwrap(), r#"{"temp": 21}"#);
    assert_eq!(ok.seen.lock().unwrap()[0].1, "https://api.example.com/v1/Oslo/forecast");
    let bad = Fake::new(404, "no such city");
    let e = call(&bad, &m, "get_forecast", &json!({"city": "Nowhere"}), &no_secrets).unwrap_err();
    assert!(e.contains("404") && e.contains("no such city"), "{e}");
    let redirect = Fake::new(302, "");
    assert!(call(&redirect, &m, "get_forecast", &json!({"city": "x"}), &no_secrets).unwrap_err().contains("redirect"));
    assert!(call(&ok, &m, "nope", &json!({}), &no_secrets).unwrap_err().contains("no tool called nope"));
}

#[test]
fn a_long_reply_is_cut_and_says_so() {
    let m = parse_manual(MANUAL, false).unwrap();
    let big = Fake::new(200, &"é".repeat(MAX_REPLY));
    let r = call(&big, &m, "get_forecast", &json!({"city": "x"}), &no_secrets).unwrap();
    assert!(r.len() < MAX_REPLY + 100 && r.ends_with("[reply cut: it was longer than 64 KB]"));
}

#[test]
fn a_manual_is_fetched_by_get_and_a_redirect_or_an_error_page_is_explained() {
    let ok = Fake::new(200, MANUAL);
    let m = fetch_manual(&ok, "https://tools.example.com/utcp").unwrap();
    assert!(!m.source_local && m.tools.len() == 2);
    assert_eq!(ok.seen.lock().unwrap()[0].0, "GET");
    assert!(fetch_manual(&Fake::new(301, ""), "https://tools.example.com/utcp").unwrap_err().contains("redirect"));
    assert!(fetch_manual(&Fake::new(500, "oops"), "https://tools.example.com/utcp").unwrap_err().contains("500"));
    assert!(fetch_manual(&ok, "http://tools.example.com/utcp").unwrap_err().contains("not https"));
    assert!(fetch_manual(&Fake::new(200, MANUAL), "http://127.0.0.1:9/utcp").unwrap().source_local, "a manual of this machine may name local tools");
}

#[test]
fn the_store_keeps_manuals_refuses_bad_names_and_addresses_and_removes() {
    let dir = std::env::temp_dir().join(format!("utcp-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let s = UtcpStore::new(&dir.join("utcp-manuals.json"));
    s.add(StoredManual { name: "weather".into(), url: "https://tools.example.com/utcp".into() }).unwrap();
    assert!(s.add(StoredManual { name: "weather".into(), url: "https://other.example.com/u".into() }).unwrap_err().contains("already"));
    assert!(s.add(StoredManual { name: "bad name".into(), url: "https://x.example.com/u".into() }).is_err());
    assert!(s.add(StoredManual { name: "plain".into(), url: "http://x.example.com/u".into() }).unwrap_err().contains("not https"));
    assert_eq!(s.list().len(), 1);
    s.remove("weather").unwrap();
    assert!(s.remove("weather").is_err() && s.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_tools_are_listed_for_the_assistant_under_the_manuals_name() {
    let m = parse_manual(MANUAL, false).unwrap();
    let t = as_tools("weather", &m);
    assert_eq!(t[0].name, "weather.get_forecast");
    assert_eq!(t[0].schema["required"][0], "city");
}
