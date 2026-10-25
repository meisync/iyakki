//! Model host on your network, against fakes only: a loopback OpenAI-style server with canned answers (streamed and plain), a
//! scripted `kubectl`, and a data folder in the temporary directory. No real network, no real cluster.

use super::*;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Models and a streamed chat with usage.
    Stream,
    /// Models and a plain JSON chat.
    Plain,
    /// 401 unless the bearer key is `secret`.
    Key,
    Html,
    BadJson,
    /// A model list that never ends (bounded by the reader).
    Huge,
    /// Accepts and says nothing.
    Silent,
    /// Models fine, chat answers 500 with an error body.
    ChatFails,
    /// An empty model list.
    NoModels,
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-modelhost-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn write_chunk(s: &mut TcpStream, data: &str) {
    let _ = s.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes());
    let _ = s.flush();
}

fn serve(mode: Mode) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { continue };
            let seen = seen2.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let (method, path) = {
                    let mut p = line.split_whitespace();
                    (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string())
                };
                let (mut len, mut bearer) = (0usize, String::new());
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    let low = h.to_ascii_lowercase();
                    if let Some(v) = low.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if low.starts_with("authorization:") {
                        bearer = h.split_once(':').unwrap().1.trim().to_string();
                    }
                }
                let mut body = vec![0u8; len];
                let _ = std::io::Read::read_exact(&mut r, &mut body);
                seen.lock().unwrap().push((method.clone(), path.clone(), String::from_utf8_lossy(&body).into_owned()));
                let json = |s: &mut TcpStream, code: &str, b: &str| {
                    let _ = s.write_all(format!("HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).as_bytes());
                };
                let models = r#"{"object":"list","data":[{"id":"nomic-embed-text","object":"model"},{"id":"llama3.2:3b","object":"model"}]}"#;
                match (mode, path.as_str()) {
                    (Mode::Silent, _) => std::thread::sleep(Duration::from_secs(3)),
                    (Mode::Html, _) => {
                        let b = "<html><body>hello</body></html>";
                        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{b}", b.len()).as_bytes());
                    }
                    (Mode::BadJson, _) => json(&mut s, "200 OK", "{\"data\": [ {"),
                    (Mode::Huge, _) => {
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                        let blob = "x".repeat(64 * 1024);
                        for _ in 0..200 {
                            write_chunk(&mut s, &blob);
                        }
                    }
                    (Mode::Key, _) if bearer != "Bearer secret" => json(&mut s, "401 Unauthorized", r#"{"error":{"message":"no key"}}"#),
                    (Mode::NoModels, "/v1/models") => json(&mut s, "200 OK", r#"{"object":"list","data":[]}"#),
                    (_, "/v1/models") => json(&mut s, "200 OK", models),
                    (Mode::ChatFails, "/v1/chat/completions") => json(&mut s, "500 Internal Server Error", r#"{"error":{"message":"model failed to load: out of memory"}}"#),
                    (Mode::Plain, "/v1/chat/completions") => {
                        std::thread::sleep(Duration::from_millis(60));
                        json(&mut s, "200 OK", r#"{"choices":[{"message":{"role":"assistant","content":"2 + 2 is 4."}}],"usage":{"completion_tokens":9}}"#)
                    }
                    (_, "/v1/chat/completions") => {
                        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
                        write_chunk(&mut s, "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n");
                        for w in ["2", " +", " 2", " is", " 4", "."] {
                            std::thread::sleep(Duration::from_millis(40));
                            write_chunk(&mut s, &format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{w}\"}}}}]}}\n\n"));
                        }
                        write_chunk(&mut s, "data: {\"choices\":[],\"usage\":{\"completion_tokens\":6}}\n\ndata: [DONE]\n\n");
                        let _ = s.write_all(b"0\r\n\r\n");
                    }
                    _ => json(&mut s, "404 Not Found", "{}"),
                }
            });
        }
    });
    Fake { port, seen }
}

fn target(f: &Fake) -> Target {
    Target { host: "127.0.0.1".into(), port: f.port }
}

fn quick() -> (Limits, Limits) {
    (Limits { connect: Duration::from_millis(500), total: Duration::from_secs(3), max_body: 1024 * 1024 }, Limits { connect: Duration::from_millis(500), total: Duration::from_secs(5), max_body: 256 * 1024 })
}

fn run(f: &Fake, key: Option<&str>) -> Result<TestReport, TestError> {
    let (a, b) = quick();
    test(&target(f), key, None, false, &a, &b)
}

// ------------------------------------------------------------------ addresses

#[test]
fn addresses_are_read_and_only_the_local_network_is_accepted_without_a_confirmation() {
    let t = |s: &str| parse_target(s, 11434, false);
    assert_eq!(t("192.168.1.20").unwrap(), Target { host: "192.168.1.20".into(), port: 11434 });
    assert_eq!(t("  192.168.1.20:8080 ").unwrap().port, 8080);
    assert_eq!(t("http://10.0.0.5:1234/v1").unwrap(), Target { host: "10.0.0.5".into(), port: 1234 });
    assert_eq!(t("http://Studio.LOCAL:8080/").unwrap(), Target { host: "studio.local".into(), port: 8080 });
    assert_eq!(t("macmini").unwrap().host, "macmini", "one word: a name on the local network");
    assert_eq!(t("172.16.4.2").unwrap().host, "172.16.4.2");
    assert_eq!(t("169.254.3.4").unwrap().host, "169.254.3.4", "link-local");
    assert_eq!(t("127.0.0.1:9").unwrap().port, 9);
    assert_eq!(t("[fe80::1]:1234").unwrap(), Target { host: "fe80::1".into(), port: 1234 });
    assert_eq!(t("[fd00::5]").unwrap().authority(), "[fd00::5]:11434");
    assert_eq!(t("192.168.1.20").unwrap().base_url(), "http://192.168.1.20:11434/v1");
    for bad in ["8.8.8.8", "1.1.1.1:11434", "172.32.0.1", "example.com", "http://models.example.org:8080", "[2001:db8::1]"] {
        let e = t(bad).unwrap_err();
        assert!(e.contains("not on your local network"), "{bad}: {e}");
    }
    // the confirmation lets one through
    assert_eq!(parse_target("8.8.8.8", 80, true).unwrap().host, "8.8.8.8");
    assert!(parse_target("", 80, false).is_err());
    assert!(parse_target("https://192.168.1.2", 80, false).unwrap_err().contains("http://"));
    assert!(parse_target("192.168.1.2:99999", 80, false).unwrap_err().contains("port"));
    assert!(parse_target("192.168.1.2:0", 80, false).is_err());
    assert!(parse_target("0.0.0.0", 80, false).unwrap_err().contains("0.0.0.0"));
    assert!(parse_target("user@192.168.1.2", 80, false).is_err());
    assert!(parse_target("bad name!", 80, false).is_err());
    assert!(parse_target("-x.local", 80, false).is_err());
    assert!(NewHost { nickname: "a".into(), preset: Preset::Mac, address: "x".into(), default_port: 1, key: String::new(), confirm: " PUBLIC ".into(), model: None }.confirm_public());
    // names are resolved and checked again; localhost is local
    assert!(resolve(&Target { host: "localhost".into(), port: 1 }, false).is_ok());
}

// ------------------------------------------------------------------ the test button

#[test]
fn a_streamed_test_lists_the_models_picks_a_chat_model_and_reports_tokens_per_second() {
    let f = serve(Mode::Stream);
    let r = run(&f, None).unwrap();
    assert_eq!(r.models, vec!["nomic-embed-text", "llama3.2:3b"]);
    assert_eq!(r.default_model.as_deref(), Some("llama3.2:3b"), "the embedding model is not the chat model");
    let c = r.chat.clone().unwrap();
    assert!(c.exact && c.tokens == 6, "{c:?}");
    assert_eq!(c.text, "2 + 2 is 4.");
    assert!(c.total_ms >= 200 && c.first_ms > 0, "{c:?}");
    let tps = c.tokens_per_s.unwrap();
    assert!(tps > 5.0 && tps < 200.0, "5 words after the first one, 40 ms apart: about 125/s at most, got {tps}");
    assert!(r.summary().contains("tokens/s") && r.summary().contains("llama3.2:3b"), "{}", r.summary());
    // the chat request asked for a tiny, streamed answer from the chosen model
    let seen = f.seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, "GET");
    assert_eq!(seen[0].1, "/v1/models");
    let body: serde_json::Value = serde_json::from_str(&seen[1].2).unwrap();
    assert_eq!((body["model"].as_str(), body["stream"].as_bool(), body["max_tokens"].as_u64()), (Some("llama3.2:3b"), Some(true), Some(48)));
}

#[test]
fn a_plain_json_chat_answer_counts_too() {
    let f = serve(Mode::Plain);
    let c = run(&f, None).unwrap().chat.unwrap();
    assert_eq!((c.tokens, c.exact, c.text.as_str()), (9, true, "2 + 2 is 4."));
    assert!(c.tokens_per_s.is_some());
}

#[test]
fn every_failure_has_its_own_plain_message() {
    // refused: nothing listens
    let e = run(&Fake { port: unused_port(), seen: Default::default() }, None).unwrap_err();
    assert!(matches!(e, TestError::Refused(_)), "{e:?}");
    assert!(e.message().contains("Nothing is listening") && e.message().contains("firewall"), "{}", e.message());
    // timeout: connected, never answered
    let f = serve(Mode::Silent);
    let (mut a, b) = quick();
    a.total = Duration::from_millis(600);
    let t = Instant::now();
    let e = test(&target(&f), None, None, false, &a, &b).unwrap_err();
    assert!(matches!(e, TestError::Timeout(_)), "{e:?}");
    assert!(t.elapsed() < Duration::from_secs(2), "bounded by the limit");
    assert!(e.message().contains("did not answer in time"));
    // not OpenAI-compatible: a web page; a 404
    let e = run(&serve(Mode::Html), None).unwrap_err();
    assert!(matches!(e, TestError::NotOpenAi(_)) && e.message().contains("not an OpenAI-compatible"), "{e:?}");
    // JSON that is not a model list
    let (g, ..) = (serve(Mode::Plain),);
    let (a, b) = quick();
    let r = http(&[format!("127.0.0.1:{}", g.port).parse().unwrap()], "x", "GET", "/v1/chat/completions", &[], None, &a).unwrap();
    assert!(matches!(parse_models(&r.body), Err(TestError::NotOpenAi(_))), "JSON without a data list");
    let _ = b;
    // 401, and the right key gets in
    let f = serve(Mode::Key);
    let e = run(&f, None).unwrap_err();
    assert_eq!(e, TestError::Unauthorized);
    assert!(e.message().contains("401") && e.message().contains("vault"));
    assert_eq!(run(&f, Some("wrong")).unwrap_err(), TestError::Unauthorized);
    assert!(run(&f, Some("secret")).unwrap().chat.is_some());
    // malformed JSON
    assert_eq!(run(&serve(Mode::BadJson), None).unwrap_err(), TestError::BadJson);
    // a huge answer is cut at the limit, not read to the end
    let t = Instant::now();
    let e = run(&serve(Mode::Huge), None).unwrap_err();
    assert_eq!(e, TestError::TooLarge);
    assert!(t.elapsed() < Duration::from_secs(3));
    // a header that promises more than the limit is refused before the body
    assert!(parse_models(b"").is_err());
}

#[test]
fn a_working_list_with_a_failing_chat_is_reported_as_reachable_with_the_reason() {
    let r = run(&serve(Mode::ChatFails), None).unwrap();
    assert!(r.chat.is_none());
    let why = r.chat_error.clone().unwrap();
    assert!(why.contains("HTTP 500") && why.contains("out of memory"), "{why}");
    assert!(r.summary().contains("the test chat failed"));
    let r = run(&serve(Mode::NoModels), None).unwrap();
    assert!(r.models.is_empty() && r.default_model.is_none() && r.chat_error.as_deref().unwrap().contains("no model"));
}

#[test]
fn tokens_per_second_is_tokens_over_the_window() {
    assert_eq!(tokens_per_second(100, 2000), Some(50.0));
    assert_eq!(tokens_per_second(1, 1000), Some(1.0));
    assert_eq!(tokens_per_second(0, 1000), None);
    assert_eq!(tokens_per_second(10, 0), None, "no window, no number");
    // streamed: the words after the first one over the time after the first one
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\n\ndata: [DONE]\n\n";
    let s = parse_chat(body, 1000, 2000).unwrap();
    assert_eq!((s.tokens, s.exact), (3, false), "counted from the stream when the server gives no usage");
    assert_eq!(s.tokens_per_s, Some(2.0), "2 words after the first, in 1 s");
    // a plain answer without usage: estimated from the text
    let s = parse_chat(br#"{"choices":[{"message":{"content":"abcdefgh"}}]}"#, 0, 500).unwrap();
    assert_eq!((s.tokens, s.exact, s.tokens_per_s), (2, false, Some(4.0)));
    assert!(parse_chat(br#"{"error":{"message":"bad model"}}"#, 0, 1).unwrap_err().contains("bad model"));
    assert!(parse_chat(b"data: [DONE]\n\n", 0, 1).unwrap_err().contains("nothing"));
    assert!(parse_chat(b"not json", 0, 1).is_err());
}

#[test]
fn chunked_bodies_are_decoded() {
    assert_eq!(decode_chunked(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"), b"hello world");
    assert_eq!(decode_chunked(b"5\r\nhel"), b"hel", "an unfinished chunk is kept as far as it came");
}

// ------------------------------------------------------------------ Models, the vault, persistence

fn services(tag: &str) -> (Arc<crate::services::Services>, PathBuf) {
    let d = tmp(tag);
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(5)));
    let s = crate::services::Services::open(&d, up.clone(), up).unwrap();
    s.on_login("root", "rootpass12", true);
    (s, d)
}

fn new_host(nick: &str, addr: &str, key: &str) -> NewHost {
    NewHost { nickname: nick.into(), preset: Preset::Mac, address: addr.into(), default_port: 11434, key: key.into(), confirm: String::new(), model: None }
}

#[test]
fn adding_registers_mac_nickname_in_models_with_the_key_in_the_vault_and_remembers_the_host() {
    let f = serve(Mode::Key);
    let (svc, d) = services("add");
    let report = run(&f, Some("secret")).unwrap();
    let n = new_host("Studio M2", &format!("127.0.0.1:{}", f.port), "secret");
    let h = add_to_models(&svc, &n, &report).unwrap();
    assert_eq!(h.id, "mac-studio-m2");
    assert_eq!((h.address.as_str(), h.port, h.label()), ("127.0.0.1", f.port, MAC_LABEL));
    let p = svc.settings.lock().unwrap().get().provider("mac-studio-m2").cloned().unwrap();
    assert_eq!((p.kind, p.base_url.as_deref()), (Kind::Compat, Some(format!("http://127.0.0.1:{}/v1", f.port).as_str())));
    assert_eq!(p.default_model.as_deref(), Some("llama3.2:3b"));
    assert_eq!(p.models, vec!["nomic-embed-text", "llama3.2:3b"]);
    assert_eq!(p.key_ref.as_deref(), Some("vault:mac-studio-m2-key"));
    assert_eq!(svc.secrets.resolve("vault:mac-studio-m2-key", None).unwrap().as_str(), "secret", "the key is in the vault, not in the settings file");
    let settings_file = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("settings")).map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>();
    assert!(!settings_file.contains("\"secret\""));
    // persisted: a new Hosts reads the same list; several Macs are fine
    let h2 = add_to_models(&svc, &new_host("Mini", "192.168.1.9", ""), &report).unwrap();
    assert_eq!(h2.id, "mac-mini");
    let list = Hosts::new(&d).list();
    assert_eq!(list.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["mac-studio-m2", "mac-mini"]);
    assert_eq!(svc.settings.lock().unwrap().get().provider("mac-mini").unwrap().key_ref, None);
    // saving the same Mac again without typing the key keeps the stored key; one provider, one entry
    add_to_models(&svc, &new_host("Studio M2", &format!("127.0.0.1:{}", f.port), ""), &report).unwrap();
    assert_eq!(svc.settings.lock().unwrap().get().provider("mac-studio-m2").unwrap().key_ref.as_deref(), Some("vault:mac-studio-m2-key"));
    assert_eq!(Hosts::new(&d).list().len(), 2);
    // a provider of that name that is not a host is never overwritten
    svc.settings.lock().unwrap().upsert_provider(ProviderCfg { base_url: Some("http://10.0.0.2/v1".into()), ..ProviderCfg::new("mac-mine", Kind::Compat) }).unwrap();
    let e = add_to_models(&svc, &new_host("mine", "192.168.1.9", ""), &report).unwrap_err();
    assert!(e.contains("already exists"), "{e}");
    // a public address needs the confirmation word
    let e = add_to_models(&svc, &new_host("far", "8.8.8.8", ""), &report).unwrap_err();
    assert!(e.contains("not on your local network"));
    let mut far = new_host("far", "8.8.8.8", "");
    far.confirm = PUBLIC_WORD.into();
    assert!(add_to_models(&svc, &far, &report).unwrap().public_ok);
    // removing takes it out of Models and the list
    remove_from_models(&svc, "mac-mini").unwrap();
    assert!(svc.settings.lock().unwrap().get().provider("mac-mini").is_none());
    assert!(Hosts::new(&d).get("mac-mini").is_none());
    assert!(remove_from_models(&svc, "mac-mine").unwrap_err().contains("not a model host"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn nicknames_become_safe_provider_ids() {
    assert_eq!(provider_id(Preset::Mac, "Studio").unwrap(), "mac-studio");
    assert_eq!(provider_id(Preset::Pc, "  Desk  Top! ").unwrap(), "host-desk-top");
    assert_eq!(provider_id(Preset::Mac, "M3 Max (office)").unwrap(), "mac-m3-max-office");
    assert!(provider_id(Preset::Mac, "!!!").is_err());
    assert!(provider_id(Preset::Mac, &"x".repeat(80)).unwrap().len() <= 32);
    assert!(crate::providers::valid_provider_id(&provider_id(Preset::Mac, "é Ünïcode").unwrap_or("mac-a".into())));
}

// ------------------------------------------------------------------ the scan

#[test]
fn the_scan_list_is_the_local_slash_24_at_most_254_hosts_and_never_another_range() {
    let own = Ipv4Addr::new(192, 168, 7, 33);
    let t = scan_targets(own, 24);
    assert_eq!(t.len(), 253, "254 hosts without this machine");
    assert!(t.iter().all(|ip| ip.octets()[..3] == [192, 168, 7]) && !t.contains(&own));
    assert!(!t.contains(&Ipv4Addr::new(192, 168, 7, 0)) && !t.contains(&Ipv4Addr::new(192, 168, 7, 255)));
    // a bigger network (a /16 or an /8) is still only the /24 around this machine
    for prefix in [8u8, 16, 20, 23] {
        let t = scan_targets(Ipv4Addr::new(10, 3, 9, 4), prefix);
        assert!(t.len() <= 254 && t.iter().all(|ip| ip.octets()[..3] == [10, 3, 9]), "/{prefix}");
    }
    // a smaller one only as far as it goes
    let t = scan_targets(Ipv4Addr::new(192, 168, 1, 21), 28);
    assert_eq!(t.len(), 13, "a /28 has 14 hosts, minus this machine");
    assert!(t.iter().all(|ip| (17..=30).contains(&ip.octets()[3])));
    // a public address gives nothing; so do loopback addresses of another host
    assert!(scan_targets(Ipv4Addr::new(8, 8, 8, 8), 24).is_empty());
    assert!(scan_targets(Ipv4Addr::new(100, 64, 0, 5), 24).is_empty(), "carrier-grade NAT is not a private LAN");
    // the ports are the four well-known ones
    assert_eq!(SCAN_PORTS, [11434, 8080, 1234, 8000]);
    // concurrency is bounded
    let o = ScanOpts { concurrency: 100_000, ..Default::default() };
    assert_eq!(worker_count(&o, 254), 64);
    assert_eq!(worker_count(&o, 3), 3);
    assert_eq!(worker_count(&ScanOpts { concurrency: 0, ..Default::default() }, 10), 1);
    assert!(ScanOpts::default().connect <= Duration::from_millis(500) && ScanOpts::default().read <= Duration::from_secs(2));
}

#[test]
fn the_scan_finds_servers_on_loopback_fakes_and_never_touches_addresses_outside_the_network() {
    let (open, keyed, html) = (serve(Mode::Stream), serve(Mode::Key), serve(Mode::Html));
    let closed = unused_port();
    let opts = ScanOpts { ports: vec![open.port, keyed.port, html.port, closed], connect: Duration::from_millis(300), read: Duration::from_millis(800), concurrency: 8 };
    let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let found = scan(&[lo], &opts, &AtomicBool::new(false));
    let ports: Vec<u16> = found.iter().map(|f| f.port).collect();
    assert!(ports.contains(&open.port) && ports.contains(&keyed.port), "{found:?}");
    assert!(!ports.contains(&html.port) && !ports.contains(&closed), "a web page and a closed port are not model servers");
    let o = found.iter().find(|f| f.port == open.port).unwrap();
    assert_eq!((o.open, o.models), (true, 2));
    assert!(!found.iter().find(|f| f.port == keyed.port).unwrap().open);
    assert!(o.line().contains("2 model(s)"));
    // addresses outside the local network are dropped before any connection is made (this returns at once, offline)
    let t = Instant::now();
    let none = scan(&[IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), "2001:db8::1".parse().unwrap()], &ScanOpts { connect: Duration::from_secs(5), ..opts.clone() }, &AtomicBool::new(false));
    assert!(none.is_empty() && t.elapsed() < Duration::from_millis(500));
    // cancelling stops it
    let stop = AtomicBool::new(true);
    assert!(scan(&[lo], &opts, &stop).is_empty());
    // at most 254 hosts are taken from a longer list
    let many: Vec<IpAddr> = (0..600).map(|_| lo).collect();
    let o2 = ScanOpts { ports: vec![closed], ..opts };
    assert!(scan(&many, &o2, &AtomicBool::new(false)).is_empty());
}

// ------------------------------------------------------------------ the cluster

fn host(addr: &str) -> Host {
    Host { id: "mac-studio".into(), nickname: "Studio".into(), preset: Preset::Mac, address: addr.into(), port: 11434, in_cluster: false, public_ok: false }
}

#[test]
fn the_service_and_endpoints_manifest_is_exact() {
    let want = "apiVersion: v1
kind: Namespace
metadata:
  name: iyakki-ai
  labels:
    app.kubernetes.io/managed-by: iyakki
---
apiVersion: v1
kind: Service
metadata:
  name: mac-studio
  namespace: iyakki-ai
  labels:
    app.kubernetes.io/managed-by: iyakki
    iyakki.dev/model-host: mac-studio
  annotations:
    iyakki.dev/note: \"remote GPU: Apple Silicon (Metal)\"
spec:
  type: ClusterIP
  ports:
    - name: http
      port: 11434
      targetPort: 11434
      protocol: TCP
---
apiVersion: v1
kind: Endpoints
metadata:
  name: mac-studio
  namespace: iyakki-ai
  labels:
    app.kubernetes.io/managed-by: iyakki
    iyakki.dev/model-host: mac-studio
subsets:
  - addresses:
      - ip: 192.168.1.20
    ports:
      - name: http
        port: 11434
        protocol: TCP
";
    assert_eq!(service_manifest(&host("192.168.1.20")), want);
    // a name gives an ExternalName Service
    let m = service_manifest(&host("studio.local"));
    assert!(m.contains("type: ExternalName\n  externalName: studio.local\n") && !m.contains("Endpoints"), "{m}");
    assert_eq!(host("10.0.0.2").cluster_url(), "http://mac-studio:11434/v1");
    let np = network_policy_example(&host("192.168.1.20"));
    assert!(np.contains("cidr: 192.168.1.20/32") && np.contains("namespace: iyakki-ai") && np.contains("port: 11434"), "{np}");
    assert!(NETWORK_POLICY_NOTE.contains("default-deny"));
}

#[derive(Default)]
struct FakeKube {
    calls: Mutex<Vec<Vec<String>>>,
    /// Phases returned by successive `get pod` calls.
    phases: Mutex<Vec<&'static str>>,
    log: String,
    apply_fails: bool,
    files: Mutex<Vec<String>>,
}

impl Kube for FakeKube {
    fn kubectl(&self, args: &[&str]) -> Out {
        self.calls.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
        let ok = |s: &str| Out { code: Some(0), stdout: s.into(), ..Default::default() };
        match args {
            ["apply", "-f", f] => {
                self.files.lock().unwrap().push(std::fs::read_to_string(f).unwrap_or_default());
                if self.apply_fails { Out { code: Some(1), stderr: "boom".into(), ..Default::default() } } else { ok("service/mac-studio created") }
            }
            [.., "get", "pod", _, "-o", _] => {
                let mut p = self.phases.lock().unwrap();
                ok(if p.len() > 1 { p.remove(0) } else { p.first().copied().unwrap_or("Pending") })
            }
            [.., "logs", _] => ok(&self.log),
            _ => ok(""),
        }
    }
}

#[test]
fn publishing_applies_the_manifest_with_the_managed_kubectl_only() {
    let d = tmp("publish");
    let k = FakeKube::default();
    let out = publish(&k, &d, &host("192.168.1.20")).unwrap();
    assert_eq!(out, "service/mac-studio created");
    let calls = k.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][0..2], ["apply".to_string(), "-f".to_string()]);
    assert!(calls[0][2].ends_with("model-hosts/mac-studio.yaml"));
    assert_eq!(k.files.lock().unwrap()[0], service_manifest(&host("192.168.1.20")));
    // a failure says why; a cluster that is not there says that
    assert!(publish(&FakeKube { apply_fails: true, ..Default::default() }, &d, &host("192.168.1.20")).unwrap_err().contains("boom"));
    struct Missing;
    impl Kube for Missing {
        fn kubectl(&self, _: &[&str]) -> Out {
            Out { code: None, missing: true, ..Default::default() }
        }
    }
    assert!(publish(&Missing, &d, &host("192.168.1.20")).unwrap_err().contains("not running"));
    // removing selects only this host's objects
    unpublish(&k, "mac-studio").unwrap();
    let last = k.calls.lock().unwrap().last().unwrap().clone();
    assert_eq!(last, vec!["-n", "iyakki-ai", "delete", "service,endpoints", "-l", "iyakki.dev/model-host=mac-studio", "--ignore-not-found"]);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_check_pod_runs_in_iyakki_ai_and_its_log_decides() {
    let d = tmp("podtest");
    let k = FakeKube { phases: Mutex::new(vec!["Pending", "Running", "Succeeded"]), log: "DNS:ok\nHTTP:ok\n{\"data\":[]}\n".into(), ..Default::default() };
    let slept = Mutex::new(0u32);
    let c = test_from_cluster(&k, &d, &host("192.168.1.20"), Duration::from_secs(60), Duration::from_secs(1), &|_| *slept.lock().unwrap() += 1).unwrap();
    assert!(c.dns && c.http);
    assert_eq!(*slept.lock().unwrap(), 2, "polled until the pod finished");
    assert!(c.message(&host("192.168.1.20")).contains("http://mac-studio:11434/v1"));
    // every call is a kubectl of the managed cluster; the pod is applied, read, logged and deleted in iyakki-ai
    let calls = k.calls.lock().unwrap().clone();
    assert!(calls.iter().all(|c| !c.iter().any(|a| a == "--kubeconfig" || a == "--context" || a == "-A")), "{calls:?}");
    assert!(calls.iter().any(|c| c.contains(&"logs".to_string())) && calls.last().unwrap().contains(&"delete".to_string()));
    let manifest = k.files.lock().unwrap()[0].clone();
    assert!(manifest.contains("namespace: iyakki-ai") && manifest.contains("name: mac-studio-check") && manifest.contains("http://mac-studio:11434/v1/models") && manifest.contains("runAsNonRoot: true") && manifest.contains("restartPolicy: Never"), "{manifest}");
    // the verdicts
    let down = parse_cluster_log("DNS:ok\nHTTP:fail\nwget: can't connect to remote host (192.168.1.20): Connection refused\n");
    assert!(down.dns && !down.http && down.detail.contains("Connection refused"));
    assert!(down.message(&host("192.168.1.20")).contains("firewall"));
    let nodns = parse_cluster_log("DNS:fail\nHTTP:fail\nbad address\n");
    assert!(!nodns.dns && nodns.message(&host("192.168.1.20")).contains("Publish the host"));
    // a pod that never finishes ends with a clear message and is cleaned up
    let k2 = FakeKube { phases: Mutex::new(vec!["Pending"]), ..Default::default() };
    let e = test_from_cluster(&k2, &d, &host("192.168.1.20"), Duration::from_millis(0), Duration::from_millis(1), &|_| {}).unwrap_err();
    assert!(e.contains("did not finish") && e.contains("busybox"), "{e}");
    assert!(k2.calls.lock().unwrap().last().unwrap().contains(&"delete".to_string()));
    let e = test_from_cluster(&FakeKube { apply_fails: true, ..Default::default() }, &d, &host("192.168.1.20"), Duration::from_secs(1), Duration::from_millis(1), &|_| {}).unwrap_err();
    assert!(e.contains("could not start"));
    let _ = std::fs::remove_dir_all(d);
}

// ------------------------------------------------------------------ the words

#[test]
fn the_guide_text_says_what_to_install_where_and_the_risks() {
    for s in ServerApp::ALL {
        let steps = s.steps(Preset::Mac).join("\n");
        assert!(steps.contains(&s.default_port().to_string()), "{s:?}");
        assert!(!s.licence().is_empty() && !s.bind_tip().is_empty());
        assert!(steps.contains("Mac"), "the Mac preset speaks of the Mac");
    }
    assert!(ServerApp::Ollama.steps(Preset::Mac).join(" ").contains("OLLAMA_HOST=0.0.0.0:11434"));
    assert!(ServerApp::LlamaCpp.steps(Preset::Mac).join(" ").contains("--host 0.0.0.0 --port 8080"));
    assert!(ServerApp::LmStudio.licence().contains("proprietary"));
    assert!(ServerApp::MlxLm.licence().starts_with("MIT") && ServerApp::Ollama.licence() == "MIT");
    assert!(!ServerApp::Ollama.steps(Preset::Pc).join(" ").contains("brew") && !ServerApp::Ollama.steps(Preset::Pc).join(" ").contains("launchctl"));
    assert!(SECURITY.contains("no login") && SECURITY.contains("firewall") && SECURITY.contains("API key"));
    assert_eq!(METAL_FACTS.len(), 5);
    assert!(METAL_FACTS.iter().any(|f| f.contains("Metal")) && METAL_FACTS.iter().any(|f| f.contains("gateway")) && METAL_FACTS.iter().any(|f| f.contains("minus about 4 GB")));
    assert_eq!(RAM_TABLE.iter().map(|r| r.0).collect::<Vec<_>>(), vec![8, 16, 24, 32, 64, 128]);
    assert_eq!(ram_table_lines().len(), 6);
    assert!(fits(16.0, 8.0, 2.0) && !fits(16.0, 11.0, 2.0) && !fits(8.0, 5.0, 1.0));
    assert_eq!(Preset::Mac.label(), "remote GPU: Apple Silicon (Metal)");
}
