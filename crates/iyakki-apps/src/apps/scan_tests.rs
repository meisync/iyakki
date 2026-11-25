use super::*;
use crate::apps::scan::ScanApp;
use iyakki_tile_scan::{base_url, provider_id, FormKind, Mode, ScanTile};
use crate::apps::scan_port::ScanPort;
use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iyakki_core_api::netscan::{net, Event as ScanEvent, Options, Service};
use iyakki_shared::providers::Kind as ProviderKind;
use iyakki_tile_api::scan::{HostInfo, Kind};
use crate::apps::scan_models::{add_model_server, CxModels};
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use crate::gfx::{rgb, Face};
use crate::tooltip::{fit_keep_ip, POINTER_DELAY};
use crate::ui::Key;
use iyakki_sec::policy::Role;
use std::io::{Read, Write};
use std::net::TcpListener;

/// A tiny Ollama look-alike on a free local port.
fn fake_ollama() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&got).lines().next().unwrap_or("").to_string();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = match path.as_str() {
                    "/" => "Ollama is running".to_string(),
                    "/api/version" => "{\"version\":\"0.5.7\"}".to_string(),
                    "/api/tags" => "{\"models\":[{\"name\":\"llama3.2:3b\"},{\"name\":\"qwen3:32b\"}]}".to_string(),
                    _ => "nope".to_string(),
                };
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            });
        }
    });
    port
}

/// A machine whose scanner is the real one (the tests scan their own listener on this computer); the rest is the fake.
fn real_scan() -> Arc<dyn iyakki_core_api::Os> {
    let os = iyakki_core_fake::FakeOs::new();
    os.netscan_fake().behind(Arc::new(iyakki_core_local::LocalNetScan));
    Arc::new(os)
}

fn admin_cx(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx.os = real_scan();
    cx
}

/// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib scan_picture -- --ignored`: a real scan of this machine, drawn.
#[test]
#[ignore]
fn scan_picture() {
    let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
    let cx = admin_cx("scanpic");
    let mut a = ScanApp::new(&cx);
    a.net_sel = a.nets.len() - 1;
    a.handle(&Event::Key(Key::Char('s')), &cx);
    finish(&mut a, &cx);
    // choose the model server
    let i = a.shown().iter().position(|h| h.has(Kind::Ollama)).unwrap_or(0);
    a.sel = i;
    let svc = a.current().and_then(|h| h.services.iter().position(|s| s.kind == Kind::Ollama)).unwrap_or(0);
    a.svc_sel = svc;
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 5);
    g.save_png(std::path::Path::new(&out)).unwrap();
}

fn finish(a: &mut ScanApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(45) {
        a.handle(&Event::Tick, cx);
        if !a.is_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the scan never finished");
}

fn scanned(cx: &AppCx) -> (ScanApp, u16) {
    let port = fake_ollama();
    let mut a = ScanApp::new(cx);
    a.ports = Some(vec![port]);
    a.net_sel = a.nets.len() - 1; // this machine
    a.handle(&Event::Key(Key::Char('s')), cx);
    assert!(a.is_running());
    finish(&mut a, cx);
    (a, port)
}

#[test]
fn the_networks_and_the_found_machines_are_the_cores_so_a_fake_machine_decides_them() {
    let mut cx = admin_cx("scan-core");
    let os = Arc::new(iyakki_core_fake::FakeOs::new());
    os.netscan_fake().set_subnets(vec![net::Subnet { iface: "fake0".into(), addr: Ipv4Addr::new(10, 9, 8, 7), prefix: 30 }]);
    cx.os = os.clone();
    let mut a = ScanApp::new(&cx);
    assert!(a.nets.iter().any(|n| n.label.contains("fake0") && n.label.contains("10.9.8.")), "{:?}", a.nets.iter().map(|n| n.label.clone()).collect::<Vec<_>>());
    a.handle(&Event::Key(Key::Char('s')), &cx);
    assert!(a.is_running());
    assert_eq!(os.netscan_fake().calls().len(), 1, "the scan was started through the core");
    finish(&mut a, &cx);
    assert!(!a.is_running());
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("netscan.start"), "{log}");
}

#[test]
fn a_scan_finds_a_model_server_and_is_written_to_the_audit_log() {
    let cx = admin_cx("scan1");
    let (a, port) = scanned(&cx);
    let hosts = a.hosts();
    assert_eq!(hosts.len(), 1);
    let s = &hosts[0].services[0];
    assert_eq!((s.port, s.kind, s.name.as_str()), (port, Kind::Ollama, "Ollama 0.5.7"));
    assert_eq!(s.models, vec!["llama3.2:3b", "qwen3:32b"]);
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("netscan.start") && log.contains("netscan.done") && log.contains("1 hosts"), "{log}");
}

#[test]
fn adding_a_found_ollama_creates_the_provider_picks_the_chat_model_and_keeps_a_key_in_the_vault() {
    let cx = admin_cx("scan2");
    let (mut a, port) = scanned(&cx);
    a.handle(&Event::Key(Key::Char('a')), &cx);
    let Mode::Form(f, _) = &mut a.mode else { panic!("the add form opens") };
    assert_eq!(f.value(0), format!("ollama-127-0-0-1-{port}"), "named after the machine, with the port when it is not the default");
    assert_eq!(f.value(1), format!("http://127.0.0.1:{port}"));
    f.set_value(2, "tok-123");
    f.fields[3].choice = 1; // qwen3:32b
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
    assert!(a.status_text().unwrap().contains("Added") && a.status_text().unwrap().contains("vault"), "{:?}", a.status_text());
    let settings = cx.settings().lock().unwrap();
    let p = &settings.get().providers[0];
    assert_eq!((p.kind, p.base_url.as_deref(), p.default_model.as_deref()), (ProviderKind::Ollama, Some(&format!("http://127.0.0.1:{port}")[..]), Some("qwen3:32b")));
    assert_eq!(p.models, vec!["llama3.2:3b", "qwen3:32b"]);
    assert_eq!(p.key_ref.as_deref(), Some(&format!("vault:ollama-127-0-0-1-{port}-key")[..]), "the provider holds a reference, not the key");
    let chat = settings.get().chat.clone().unwrap();
    assert_eq!((chat.provider.as_str(), chat.model.as_str()), (&format!("ollama-127-0-0-1-{port}")[..], "qwen3:32b"), "the first model server becomes the chat model");
    drop(settings);
    assert!(cx.vault().system_names().unwrap().contains(&format!("ollama-127-0-0-1-{port}-key")));
    let json = std::fs::read_to_string(cx.data_dir().join("settings.json")).unwrap_or_default();
    assert!(!json.contains("tok-123"), "the key is not in the settings file");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    assert!(log.contains("settings.provider.save") && log.contains("vault.system.set"));
}

#[test]
fn a_lemonade_found_on_the_network_is_added_as_lemonade_with_only_its_chat_models_in_the_chat_list() {
    let fake = crate::apps::lemonade_fake::serve();
    let mut cx = cx_with_admin_store("scanlem");
    let d = temp_dir("scanlem-svc");
    cx.services = services_in(&d, Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(10))));
    cx.services.on_login("root", "rootpass12", true);
    cx.os = real_scan();
    let mut a = ScanApp::new(&cx);
    a.ports = Some(vec![fake.port]);
    a.net_sel = a.nets.len() - 1;
    a.handle(&Event::Key(Key::Char('s')), &cx);
    finish(&mut a, &cx);
    let s = a.hosts()[0].services[0].clone();
    assert_eq!((s.kind, s.name.as_str()), (Kind::LlmServer, "Lemonade Server 2026.42.1"));
    a.handle(&Event::Key(Key::Char('a')), &cx);
    let Mode::Form(f, _) = &mut a.mode else { panic!("the add form opens") };
    assert_eq!((f.value(0), f.value(1)), (format!("lemonade-127-0-0-1-{}", fake.port), fake.base()));
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
    assert!(a.status_text().unwrap().contains("(Lemonade)") && a.status_text().unwrap().contains("2 chat models"), "{:?}", a.status_text());
    let st = cx.settings().lock().unwrap().get().clone();
    let p = &st.providers[0];
    assert_eq!((p.kind, p.models.clone()), (ProviderKind::Lemonade, vec!["Qwen3-0.6B-GGUF".to_string(), "Phi-4-Multimodal-GGUF".to_string()]));
    assert_eq!(p.model_kinds.get("Whisper-Base").map(String::as_str), Some("stt"));
    assert_eq!(st.chat.as_ref().map(|c| c.model.as_str()), Some("Qwen3-0.6B-GGUF"), "the first chat model, never a picture model");
}

#[test]
fn a_second_model_server_does_not_take_over_the_chat_model_and_bad_names_are_refused() {
    let cx = admin_cx("scan3");
    let (mut a, _) = scanned(&cx);
    a.handle(&Event::Key(Key::Char('a')), &cx);
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    let first = cx.settings().lock().unwrap().get().chat.clone().unwrap().provider;
    a.handle(&Event::Key(Key::Char('a')), &cx);
    let Mode::Form(f, _) = &mut a.mode else { panic!() };
    f.set_value(0, "Bad Name!");
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    let Mode::Form(f, _) = &mut a.mode else { panic!("the form stays open") };
    assert!(f.error.as_deref().unwrap().contains("Provider name"), "{:?}", f.error);
    f.set_value(0, "second");
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    let s = cx.settings().lock().unwrap();
    assert_eq!(s.get().providers.len(), 2);
    assert_eq!(s.get().chat.clone().unwrap().provider, first, "the first choice stays");
}

#[test]
fn only_administrators_add_models_and_the_hosts_a_scan_may_touch_follow_the_policy() {
    let mut cx = admin_cx("scan4");
    let (mut a, _) = scanned(&cx);
    cx.session.role = Role::Operator;
    cx.session.policy = Role::Operator.template();
    a.handle(&Event::Key(Key::Char('a')), &cx);
    assert!(matches!(a.mode, Mode::Browse));
    assert!(a.status_text().unwrap().contains("administrator"), "{:?}", a.status_text());
    assert!(cx.settings().lock().unwrap().get().providers.is_empty());
    // copying the address needs no permission
    a.handle(&Event::Key(Key::Char('c')), &cx);
    assert!(a.status_text().unwrap().starts_with("Copied http://127.0.0.1:"));
    // an account that may not reach those addresses cannot scan them
    cx.session.policy.net_hosts = vec!["other.example".into()];
    let mut b = ScanApp::new(&cx);
    b.net_sel = b.nets.len() - 1;
    b.handle(&Event::Key(Key::Char('s')), &cx);
    assert!(!b.is_running() && b.status_text().unwrap().contains("may not scan"), "{:?}", b.status_text());
    // the terminal level is checked in the command; here the tile is simply not given to users
    assert!(!Role::User.template().apps.contains(&iyakki_sec::policy::App::Scan) && Role::Operator.template().apps.contains(&iyakki_sec::policy::App::Scan));
}

#[test]
fn typed_targets_are_checked_filters_work_and_every_state_draws() {
    let cx = admin_cx("scan5");
    let (mut a, _) = scanned(&cx);
    a.handle(&Event::Key(Key::Char('t')), &cx);
    for c in "8.8.8.8".chars() {
        a.handle(&Event::Key(Key::Char(c)), &cx);
    }
    a.handle(&Event::Key(Key::Ctrl('s')), &cx);
    let Mode::Form(f, _) = &a.mode else { panic!() };
    assert!(f.error.as_deref().unwrap().contains("not on the local network"), "{:?}", f.error);
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(a.mode, Mode::Browse));
    // filters
    a.handle(&Event::Key(Key::Char('4')), &cx); // media: nothing
    assert!(a.shown().is_empty());
    a.handle(&Event::Key(Key::Char('3')), &cx); // cameras: nothing
    assert!(a.shown().is_empty());
    a.handle(&Event::Key(Key::Char('2')), &cx); // model servers
    assert_eq!(a.shown().len(), 1);
    a.handle(&Event::Key(Key::Char('1')), &cx); // all
    assert_eq!(a.shown().len(), 1);
    let mut g;
    for (w, h) in [(1280, 720), (800, 600), (1920, 1080)] {
        g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), (w as f32 / 1280.0).clamp(0.6, 3.0), &cx, 3);
    }
    a.mode = Mode::Form(ScanTile::target_form(), FormKind::Target);
    g = Gfx::new(1280, 720);
    a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 3);
    assert!(!a.help().is_empty());
}

#[test]
fn openai_compatible_servers_are_added_as_compatible_providers_through_their_v1_address() {
    let cx = admin_cx("scan6");
    let mut svc = iyakki_tile_api::scan::Service::new(1234, Kind::LlmServer, "LM Studio");
    svc.url = Some("http://192.168.1.50:1234/v1".into());
    svc.models = vec!["qwen2-7b".into()];
    let ip: Ipv4Addr = "192.168.1.50".parse().unwrap();
    assert_eq!(provider_id(ip, &svc), "llm-192-168-1-50");
    assert_eq!(base_url(ip, &svc), "http://192.168.1.50:1234/v1");
    let msg = add_model_server(&CxModels(&cx), ip, &svc, "lmstudio", &base_url(ip, &svc), "", "").unwrap();
    assert!(msg.contains("chat model"), "{msg}");
    let s = cx.settings().lock().unwrap();
    assert_eq!((s.get().providers[0].kind, s.get().providers[0].default_model.as_deref()), (ProviderKind::Compat, Some("qwen2-7b")));
    assert!(s.get().providers[0].key_ref.is_none());
}

#[test]
fn the_devices_view_lists_strangers_first_and_an_administrator_trusts_names_and_forgets_them() {
    let cx = cx_with_admin_store("scan-devices");
    let mut app = ScanApp::new(&cx);
    let n = |ip: &str, mac: &str| iyakki_shared::devwatch::Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() };
    cx.devwatch().observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01")], 100);
    cx.devwatch().observe(&[n("10.0.0.1", "aa:aa:aa:aa:aa:01"), n("10.0.0.9", "bb:bb:bb:bb:bb:09")], 200);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(110)), "the Devices button is there and says one is new");
    app.handle(&Event::Key(Key::Char('d')), &cx);
    assert!(app.devices);
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
    assert_eq!(g.hits.iter().filter(|h| h.label == "device").count(), 2);
    // the stranger is first
    assert_eq!(cx.devwatch().list()[0].mac, "bb:bb:bb:bb:bb:09");
    app.handle(&Event::Key(Key::Char('r')), &cx);
    for c in "Guest phone".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(cx.devwatch().list().iter().find(|d| d.mac == "bb:bb:bb:bb:bb:09").unwrap().name, "Guest phone");
    app.handle(&Event::Key(Key::Char('t')), &cx);
    assert_eq!(cx.devwatch().untrusted_count(), 0);
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("netsec.trust") && l.contains("bb:bb")));
    // forget the chosen one (the list is trusted-last now, so choose the stranger by row)
    let row = cx.devwatch().list().iter().position(|d| d.mac == "bb:bb:bb:bb:bb:09").unwrap();
    app.handle(&Event::Row(row), &cx);
    app.handle(&Event::Key(Key::Char('x')), &cx);
    assert_eq!(cx.devwatch().list().len(), 1);
    // switching the watch off
    app.handle(&Event::Key(Key::Char('w')), &cx);
    assert!(!cx.devwatch().enabled());
    // Esc leaves the devices view first, then the app
    assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Stay));
    assert!(!app.devices);
    // an ordinary account can look but not change anything
    let mut cx2 = cx_with_admin_store("scan-devices2");
    cx2.services = cx.services.clone();
    cx2.session.role = Role::User;
    cx2.session.policy = Role::User.template();
    cx2.services.devwatch().set_enabled(true);
    let mut u = ScanApp::new(&cx2);
    u.handle(&Event::Key(Key::Char('d')), &cx2);
    u.handle(&Event::Key(Key::Char('T')), &cx2);
    assert!(u.status_text().unwrap().contains("Only administrators"));
}

#[test]
fn a_narrow_machine_list_keeps_the_address_and_a_tooltip_has_all_of_the_row() {
    let cx = admin_cx("scan-tip");
    let mut a = ScanApp::new(&cx);
    let ip = Ipv4Addr::new(192, 168, 100, 200);
    let mut h = HostInfo::new(ip);
    h.name = Some("living-room-television-with-a-very-long-friendly-name".into());
    h.mac = Some("aa:bb:cc:dd:ee:ff".into());
    a.hosts.insert(ip, h);
    let area = R::new(0.0, 46.0, 520.0, 674.0);
    let mut g = Gfx::new(520, 720);
    let draw = |a: &mut ScanApp, g: &mut Gfx| {
        g.clear(rgb(9, 11, 20));
        a.render(g, area, 1.0, &cx, 5);
    };
    draw(&mut a, &mut g);
    let row = g.hits.iter().find(|h| h.label == "192.168.100.200").expect("the row").rect;
    // the line is cut for the width, yet the address is whole
    let head = format!("{ip}  {}", a.hosts[&ip].name.clone().unwrap());
    let cut = fit_keep_ip(&mut g, 16.0, &head, Face::Bold, row.w - 60.0);
    assert!(cut != head && cut.starts_with("192.168.100.200"), "{cut}");
    let plain = g.rgba().to_vec();
    // the pointer rests on it: nothing for half a second, then the tooltip, which asks for a new picture
    let (mx, my) = ((row.x + 20.0) as i32, (row.y + 10.0) as i32);
    g.mouse = (mx, my);
    a.handle(&Event::MouseMove(mx, my), &cx);
    for _ in 0..POINTER_DELAY - 1 {
        a.handle(&Event::Tick, &cx);
    }
    draw(&mut a, &mut g);
    let hovered = g.rgba().to_vec();
    for _ in 0..2 {
        a.handle(&Event::Tick, &cx);
    }
    assert!(a.wants_redraw(), "the tooltip appears");
    draw(&mut a, &mut g);
    assert_ne!(hovered, g.rgba(), "the tooltip is drawn");
    assert_ne!(plain, g.rgba());
    // a key press puts it away at once
    a.handle(&Event::Key(Key::Down), &cx);
    draw(&mut a, &mut g);
    assert!(a.wants_redraw());
}

// ---- the tile on a fake port: no shell context, no real network, no real settings -----------------------------------------------------------

use iyakki_shared::devwatch::{Device, Neighbor};
use std::net::Ipv4Addr as Ip;
use std::sync::Mutex as StdMutex;

#[derive(Default)]
struct FakeWatch {
    devices: StdMutex<Vec<Device>>,
    on: StdMutex<bool>,
    observed: StdMutex<Vec<Vec<Neighbor>>>,
    trusted: StdMutex<Vec<String>>,
    forgotten: StdMutex<Vec<String>>,
}

impl iyakki_agent_api::DevWatchApi for FakeWatch {
    fn enabled(&self) -> bool {
        *self.on.lock().unwrap()
    }
    fn set_enabled(&self, on: bool) {
        *self.on.lock().unwrap() = on;
    }
    fn observe(&self, _seen: &[Neighbor], _now: i64) -> Vec<Device> {
        vec![]
    }
    fn observe_scan(&self, seen: &[Neighbor], _now: i64) -> Vec<Device> {
        self.observed.lock().unwrap().push(seen.to_vec());
        vec![]
    }
    fn take_fresh(&self) -> Vec<Device> {
        vec![]
    }
    fn list(&self) -> Vec<Device> {
        self.devices.lock().unwrap().clone()
    }
    fn untrusted_count(&self) -> usize {
        self.devices.lock().unwrap().iter().filter(|d| !d.trusted).count()
    }
    fn trust(&self, mac: &str) -> bool {
        self.trusted.lock().unwrap().push(mac.into());
        true
    }
    fn trust_all(&self) -> usize {
        self.devices.lock().unwrap().iter().filter(|d| !d.trusted).count()
    }
    fn rename(&self, _mac: &str, _name: &str) -> bool {
        true
    }
    fn forget(&self, mac: &str) -> bool {
        self.forgotten.lock().unwrap().push(mac.into());
        true
    }
}

struct FakePort {
    admin: bool,
    allow: bool,
    start_error: Option<String>,
    started: StdMutex<Vec<Vec<Ip>>>,
    watch: Arc<FakeWatch>,
    audit: StdMutex<Vec<(String, String)>>,
}

impl FakePort {
    fn new() -> FakePort {
        FakePort { admin: true, allow: true, start_error: None, started: StdMutex::new(vec![]), watch: Arc::new(FakeWatch::default()), audit: StdMutex::new(vec![]) }
    }
    fn audit_text(&self) -> String {
        self.audit.lock().unwrap().iter().map(|(a, s)| format!("{a} {s}")).collect::<Vec<_>>().join("\n")
    }
}

impl ScanPort for FakePort {
    fn subnets(&self) -> Vec<net::Subnet> {
        vec![net::Subnet { iface: "eth0".into(), addr: Ip::new(192, 168, 7, 5), prefix: 30 }]
    }
    fn start_scan(&self, targets: Vec<Ip>, _opts: Options, _cancel: Arc<AtomicBool>, tx: std::sync::mpsc::Sender<ScanEvent>) -> Result<(), String> {
        if let Some(e) = &self.start_error {
            return Err(e.clone());
        }
        self.started.lock().unwrap().push(targets.clone());
        // the answers are put on the channel at once: no thread, no clock
        let mut h = iyakki_core_api::netscan::HostInfo::new(targets[0]);
        h.mac = Some("AA:BB:CC:00:11:22".into());
        tx.send(ScanEvent::Started { hosts: targets.len(), ports: 1 }).unwrap();
        tx.send(ScanEvent::Host(h)).unwrap();
        tx.send(ScanEvent::Finished { hosts: 1, services: 0, cancelled: false }).unwrap();
        Ok(())
    }
    fn allows_host(&self, _ip: &str) -> bool {
        self.allow
    }
    fn admin(&self) -> bool {
        self.admin
    }
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi> {
        self.watch.clone()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.lock().unwrap().push((action.into(), subject.into()));
    }
    fn add_model_server(&self, _ip: Ip, _svc: &Service, id: &str, _address: &str, _model: &str, _key: &str) -> Result<String, String> {
        Ok(format!("Added {id}"))
    }
}

#[test]
fn fake_port_the_networks_come_from_the_port_and_this_machine_is_always_last() {
    let a = ScanApp::with_port(&FakePort::new());
    assert_eq!(a.nets.len(), 2);
    assert!(a.nets[0].label.starts_with("eth0  192.168.7.4/30"), "{}", a.nets[0].label);
    assert_eq!(a.nets[1].targets, vec![Ip::LOCALHOST]);
}

#[test]
fn fake_port_a_scan_asks_the_scanner_audits_start_and_finish_and_tells_the_device_watch_for_administrators() {
    let port = FakePort::new();
    let mut a = ScanApp::with_port(&port);
    a.start(&port);
    assert!(a.is_running());
    a.poll(&port);
    assert!(!a.is_running());
    assert_eq!(a.hosts().len(), 1);
    assert_eq!(port.started.lock().unwrap().len(), 1);
    let t = port.audit_text();
    assert!(t.contains("netscan.start eth0") && t.contains("netscan.done 1 hosts, 0 services"), "{t}");
    let seen = port.watch.observed.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0][0].mac, "aa:bb:cc:00:11:22", "the address is lower-cased for the watch");
}

#[test]
fn fake_port_an_ordinary_account_does_not_feed_the_device_watch() {
    let mut port = FakePort::new();
    port.admin = false;
    let mut a = ScanApp::with_port(&port);
    a.start(&port);
    a.poll(&port);
    assert!(port.watch.observed.lock().unwrap().is_empty());
    assert!(port.audit_text().contains("netscan.done"));
}

#[test]
fn fake_port_addresses_the_policy_refuses_are_never_scanned() {
    let mut port = FakePort::new();
    port.allow = false;
    let mut a = ScanApp::with_port(&port);
    a.start(&port);
    assert!(!a.is_running());
    assert!(port.started.lock().unwrap().is_empty());
    assert!(a.status_text().unwrap().starts_with("Your account may not scan 192.168.7."), "{:?}", a.status_text());
    assert!(port.audit.lock().unwrap().is_empty());
}

#[test]
fn fake_port_a_scanner_that_refuses_says_why_and_nothing_is_audited() {
    let mut port = FakePort::new();
    port.start_error = Some("net.scan was not allowed".into());
    let mut a = ScanApp::with_port(&port);
    a.start(&port);
    assert!(!a.is_running());
    assert_eq!(a.status_text(), Some("The scan did not start: net.scan was not allowed"));
    assert!(port.audit.lock().unwrap().is_empty());
}

#[test]
fn fake_port_the_device_list_changes_only_for_administrators_and_each_change_is_audited() {
    let mut port = FakePort::new();
    port.watch.devices.lock().unwrap().push(Device { mac: "aa:bb:cc:00:11:22".into(), ip: "192.168.7.4".into(), iface: "eth0".into(), first_seen: 1, last_seen: 2, trusted: false, name: String::new() });
    let mut a = ScanApp::with_port(&port);
    a.handle_devices(&Event::Key(Key::Char('t')), &port);
    assert_eq!(port.watch.trusted.lock().unwrap().as_slice(), ["aa:bb:cc:00:11:22"]);
    a.handle_devices(&Event::Key(Key::Char('w')), &port);
    assert!(*port.watch.on.lock().unwrap());
    let t = port.audit_text();
    assert!(t.contains("netsec.trust aa:bb:cc:00:11:22") && t.contains("netsec.watch on"), "{t}");
    port.admin = false;
    a.handle_devices(&Event::Key(Key::Char('x')), &port);
    assert!(port.watch.forgotten.lock().unwrap().is_empty());
    assert_eq!(a.status_text(), Some("Only administrators can change the device list."));
}

