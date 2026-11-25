//! The NET SCAN tile on a fake port: no shell context, no scanner, no settings, no clock. The scan's answers are put in a queue the tile takes
//! on its next poll; the time is a number the test sets.

use std::collections::VecDeque;
use std::net::Ipv4Addr as Ip;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use iyakki_tile_api::scan::{Confidence, DevWatch, Device, Event as Ev, HostInfo, Kind, Neighbor, ScanFeed, ScanOpts, Service, Subnet};
use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::tooltip::{fit_keep_ip, POINTER_DELAY};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

#[derive(Default)]
struct Watch {
    devices: Mutex<Vec<Device>>,
    on: Mutex<bool>,
    observed: Mutex<Vec<Vec<Neighbor>>>,
    renamed: Mutex<Vec<(String, String)>>,
    trusted: Mutex<Vec<String>>,
    forgotten: Mutex<Vec<String>>,
}

impl DevWatch for Watch {
    fn enabled(&self) -> bool {
        *self.on.lock().unwrap()
    }
    fn set_enabled(&self, on: bool) {
        *self.on.lock().unwrap() = on;
    }
    fn observe_scan(&self, seen: &[Neighbor], _now: i64) {
        self.observed.lock().unwrap().push(seen.to_vec());
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
    fn rename(&self, mac: &str, name: &str) -> bool {
        self.renamed.lock().unwrap().push((mac.into(), name.into()));
        mac != "gone"
    }
    fn forget(&self, mac: &str) -> bool {
        self.forgotten.lock().unwrap().push(mac.into());
        true
    }
}

struct Feed(Arc<Mutex<VecDeque<Ev>>>);

impl ScanFeed for Feed {
    fn next(&mut self) -> Option<Ev> {
        self.0.lock().unwrap().pop_front()
    }
}

struct Port {
    admin: bool,
    allow: bool,
    start_error: Option<String>,
    queue: Arc<Mutex<VecDeque<Ev>>>,
    started: Mutex<Vec<(Vec<Ip>, ScanOpts)>>,
    cancels: Mutex<Vec<Arc<AtomicBool>>>,
    watch: Arc<Watch>,
    audit: Mutex<Vec<(String, String)>>,
    added: Mutex<Vec<(Ip, String, String, String, String)>>,
    add_result: Mutex<Result<String, String>>,
    now: i64,
}

impl Port {
    fn new() -> Port {
        Port { admin: true, allow: true, start_error: None, queue: Arc::default(), started: Mutex::new(vec![]), cancels: Mutex::new(vec![]), watch: Arc::new(Watch::default()), audit: Mutex::new(vec![]), added: Mutex::new(vec![]), add_result: Mutex::new(Ok("Added".into())), now: 10_000 }
    }
    fn say(&self, ev: Ev) {
        self.queue.lock().unwrap().push_back(ev);
    }
    fn audit_text(&self) -> String {
        self.audit.lock().unwrap().iter().map(|(a, s)| format!("{a} {s}")).collect::<Vec<_>>().join("\n")
    }
}

fn host(ip: Ip, svcs: Vec<Service>) -> HostInfo {
    let mut h = HostInfo::new(ip);
    h.services = svcs;
    h
}

fn ollama() -> Service {
    let mut s = Service::new(11434, Kind::Ollama, "Ollama 0.5.7");
    s.models = vec!["llama3.2:3b".into(), "qwen3:32b".into()];
    s.url = Some("http://192.168.7.6:11434".into());
    s
}

impl ScanPort for Port {
    fn subnets(&self) -> Vec<Subnet> {
        vec![Subnet { iface: "eth0".into(), addr: Ip::new(192, 168, 7, 5), prefix: 30 }]
    }
    fn hosts_of(&self, s: &Subnet) -> Vec<Ip> {
        // a /30 has two hosts besides this machine's own address: .4 is the network, .6 the other host
        vec![Ip::new(192, 168, 7, 6)].into_iter().filter(|ip| *ip != s.addr).collect()
    }
    fn parse_targets(&self, text: &str) -> Result<Vec<Ip>, String> {
        text.parse::<Ip>().map(|ip| vec![ip]).map_err(|_| format!("{text} is not an IPv4 address"))
    }
    fn start_scan(&self, targets: Vec<Ip>, opts: ScanOpts, cancel: Arc<AtomicBool>) -> Result<Box<dyn ScanFeed>, String> {
        if let Some(e) = &self.start_error {
            return Err(e.clone());
        }
        self.started.lock().unwrap().push((targets, opts));
        self.cancels.lock().unwrap().push(cancel);
        Ok(Box::new(Feed(self.queue.clone())))
    }
    fn allows_host(&self, _ip: &str) -> bool {
        self.allow
    }
    fn admin(&self) -> bool {
        self.admin
    }
    fn devwatch(&self) -> Arc<dyn DevWatch> {
        self.watch.clone()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.lock().unwrap().push((action.into(), subject.into()));
    }
    fn add_model_server(&self, ip: Ip, _svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
        self.added.lock().unwrap().push((ip, id.into(), address.into(), model.into(), key.into()));
        self.add_result.lock().unwrap().clone()
    }
    fn now(&self) -> i64 {
        self.now
    }
}

fn key(t: &mut ScanTile, p: &Port, k: Key) -> Answer {
    t.handle(&Event::Key(k), p)
}

fn type_text(t: &mut ScanTile, p: &Port, s: &str) {
    for c in s.chars() {
        key(t, p, Key::Char(c));
    }
}

/// A tile that has scanned and found a model server at 192.168.7.6.
fn scanned(p: &Port) -> ScanTile {
    let mut t = ScanTile::new(p);
    p.say(Ev::Started { hosts: 1, ports: 2 });
    p.say(Ev::Progress { done: 1, total: 2 });
    p.say(Ev::Host(host(Ip::new(192, 168, 7, 6), vec![ollama()])));
    p.say(Ev::Finished { hosts: 1, services: 1, cancelled: false });
    key(&mut t, p, Key::Char('s'));
    t.handle(&Event::Tick, p);
    t
}

fn draw(t: &mut ScanTile, g: &mut Gfx, p: &Port, w: f32, h: f32, tick: u64) {
    g.clear(rgb(9, 11, 20));
    t.render(g, R::new(0.0, 46.0, w, h - 46.0), (w / 1280.0).clamp(0.6, 3.0), p, tick);
}

// ---- the networks and a scan

#[test]
fn the_networks_come_from_the_port_and_this_machine_is_always_last() {
    let t = ScanTile::new(&Port::new());
    assert_eq!(t.nets.len(), 2);
    assert!(t.nets[0].label.starts_with("eth0  192.168.7.4/30  (1 addresses)"), "{}", t.nets[0].label);
    assert_eq!(t.nets[1].targets, vec![Ip::LOCALHOST]);
    assert_eq!(t.nets[1].label, "This machine (127.0.0.1)");
}

#[test]
fn a_scan_asks_the_scanner_audits_start_and_finish_and_follows_the_events() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    p.say(Ev::Started { hosts: 1, ports: 4 });
    key(&mut t, &p, Key::Enter);
    assert!(t.running && t.status.is_none());
    assert_eq!(p.started.lock().unwrap()[0].0, vec![Ip::new(192, 168, 7, 6)]);
    assert_eq!(p.started.lock().unwrap()[0].1, ScanOpts::default(), "the scanner's own port list unless the test sets one");
    t.handle(&Event::Tick, &p);
    assert_eq!(t.progress, (0, 4), "started: the probes to ask");
    p.say(Ev::Progress { done: 3, total: 4 });
    t.handle(&Event::Tick, &p);
    assert_eq!((t.progress, t.running), ((3, 4), true));
    p.say(Ev::Host(host(Ip::new(192, 168, 7, 6), vec![ollama()])));
    p.say(Ev::Finished { hosts: 1, services: 1, cancelled: false });
    t.handle(&Event::Tick, &p);
    assert!(!t.running && t.feed.is_none());
    assert_eq!(t.summary, Some((1, 1, false)));
    assert_eq!(t.hosts().len(), 1);
    let log = p.audit_text();
    assert!(log.contains("netscan.start eth0") && log.contains("netscan.done 1 hosts, 1 services") && !log.contains("stopped"), "{log}");
}

#[test]
fn stopping_sets_the_cancel_flag_and_a_stopped_scan_is_audited_as_stopped() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('s'));
    key(&mut t, &p, Key::Char('x'));
    assert!(p.cancels.lock().unwrap()[0].load(Ordering::Relaxed));
    p.say(Ev::Finished { hosts: 0, services: 0, cancelled: true });
    t.handle(&Event::Tick, &p);
    assert_eq!(t.summary, Some((0, 0, true)));
    assert!(p.audit_text().contains("netscan.done 0 hosts, 0 services, stopped"));
}

#[test]
fn leaving_while_a_scan_runs_stops_it_and_a_second_start_while_running_does_nothing() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('s'));
    key(&mut t, &p, Key::Char('s'));
    assert_eq!(p.started.lock().unwrap().len(), 1);
    assert_eq!(key(&mut t, &p, Key::Esc), Answer::Back);
    assert!(p.cancels.lock().unwrap()[0].load(Ordering::Relaxed));
}

#[test]
fn only_administrators_feed_the_device_watch_and_it_gets_the_lower_cased_hardware_addresses_of_the_hosts_that_have_one() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    let mut with_mac = host(Ip::new(192, 168, 7, 6), vec![]);
    with_mac.mac = Some("AA:BB:CC:00:11:22".into());
    p.say(Ev::Host(with_mac));
    p.say(Ev::Host(host(Ip::new(192, 168, 7, 7), vec![])));
    p.say(Ev::Finished { hosts: 2, services: 0, cancelled: false });
    key(&mut t, &p, Key::Char('s'));
    t.handle(&Event::Tick, &p);
    let seen = p.watch.observed.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], vec![Neighbor { ip: "192.168.7.6".into(), mac: "aa:bb:cc:00:11:22".into(), iface: "scan".into() }]);
    drop(seen);
    let mut q = Port::new();
    q.admin = false;
    let mut u = ScanTile::new(&q);
    q.say(Ev::Finished { hosts: 0, services: 0, cancelled: false });
    key(&mut u, &q, Key::Char('s'));
    u.handle(&Event::Tick, &q);
    assert!(q.watch.observed.lock().unwrap().is_empty());
    assert!(q.audit_text().contains("netscan.done"));
}

#[test]
fn addresses_the_policy_refuses_are_never_scanned_and_nothing_is_audited() {
    let mut p = Port::new();
    p.allow = false;
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('s'));
    assert!(!t.running && p.started.lock().unwrap().is_empty() && p.audit.lock().unwrap().is_empty());
    assert!(t.status_text().unwrap().starts_with("Your account may not scan 192.168.7.6"), "{:?}", t.status_text());
}

#[test]
fn a_scanner_that_refuses_says_why_and_nothing_is_audited() {
    let mut p = Port::new();
    p.start_error = Some("net.scan was not allowed".into());
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('s'));
    assert!(!t.running && t.feed.is_none());
    assert_eq!(t.status_text(), Some("The scan did not start: net.scan was not allowed"));
    assert!(p.audit.lock().unwrap().is_empty());
}

#[test]
fn n_changes_the_network_only_while_nothing_runs_and_the_test_port_list_goes_to_the_scanner() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    t.ports = Some(vec![8080]);
    key(&mut t, &p, Key::Char('n'));
    assert_eq!(t.net_sel, 1);
    key(&mut t, &p, Key::Char('s'));
    assert_eq!(p.started.lock().unwrap()[0].0, vec![Ip::LOCALHOST]);
    assert_eq!(p.started.lock().unwrap()[0].1.ports, Some(vec![8080]));
    key(&mut t, &p, Key::Char('n'));
    assert_eq!(t.net_sel, 1, "not while a scan runs");
}

#[test]
fn a_typed_target_is_checked_by_the_port_and_a_good_one_is_scanned_at_once() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('t'));
    type_text(&mut t, &p, "nonsense");
    key(&mut t, &p, Key::Ctrl('s'));
    let Mode::Form(f, _) = &t.mode else { panic!("the form stays open") };
    assert_eq!(f.error.as_deref(), Some("nonsense is not an IPv4 address"));
    key(&mut t, &p, Key::Esc);
    assert!(matches!(t.mode, Mode::Browse));
    key(&mut t, &p, Key::Char('t'));
    type_text(&mut t, &p, "192.168.7.20");
    key(&mut t, &p, Key::Ctrl('s'));
    assert!(t.running && matches!(t.mode, Mode::Browse));
    assert_eq!(p.started.lock().unwrap()[0].0, vec![Ip::new(192, 168, 7, 20)]);
    assert_eq!(t.net_sel, 0);
    assert!(t.nets[0].label.starts_with("typed: 192.168.7.20  (1 addresses)"), "{}", t.nets[0].label);
}

// ---- filters, the list and the pictures

fn mixed() -> ScanTile {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    let mut cam = Service::new(554, Kind::Camera, "Camera");
    cam.risk = Some("no password".into());
    for h in [
        host(Ip::new(192, 168, 7, 6), vec![ollama()]),
        host(Ip::new(192, 168, 7, 7), vec![cam]),
        host(Ip::new(192, 168, 7, 8), vec![Service::new(8096, Kind::Media, "Jellyfin")]),
        host(Ip::new(192, 168, 7, 9), vec![Service::new(9100, Kind::Printer, "Printer")]),
        host(Ip::new(192, 168, 7, 10), vec![Service::new(80, Kind::Web, "nginx")]),
    ] {
        t.hosts.insert(h.ip, h);
    }
    t
}

#[test]
fn each_filter_shows_its_kind_of_machine_and_the_keys_and_the_tabs_choose_it() {
    let p = Port::new();
    let mut t = mixed();
    let n = |t: &ScanTile| t.shown().len();
    assert_eq!(n(&t), 5);
    for (k, want) in [('2', 1), ('3', 1), ('4', 1), ('5', 1), ('6', 1), ('1', 5)] {
        key(&mut t, &p, Key::Char(k));
        assert_eq!(n(&t), want, "filter {k}");
    }
    t.handle(&Event::Tab(2), &p);
    assert_eq!((t.filter, n(&t)), (2, 1));
    key(&mut t, &p, Key::Char('f'));
    assert_eq!(t.filter, 3);
    t.filter = FILTERS.len() - 1;
    key(&mut t, &p, Key::Char('f'));
    assert_eq!(t.filter, 0, "the filter wraps round");
}

#[test]
fn the_cursor_moves_over_machines_and_services_and_stays_inside_the_lists() {
    let p = Port::new();
    let mut t = mixed();
    key(&mut t, &p, Key::Down);
    key(&mut t, &p, Key::Down);
    assert_eq!(t.sel, 2);
    for _ in 0..10 {
        key(&mut t, &p, Key::Down);
    }
    assert_eq!(t.sel, 4);
    t.handle(&Event::Scroll(100), &p);
    assert_eq!(t.sel, 0);
    t.handle(&Event::Row(99), &p);
    assert_eq!(t.sel, 4, "a row past the end is the last one");
    key(&mut t, &p, Key::Up);
    assert_eq!(t.sel, 3);
    key(&mut t, &p, Key::Right);
    assert_eq!(t.svc_sel, 0, "the machine has one service");
    key(&mut t, &p, Key::Left);
    assert_eq!(t.svc_sel, 0);
}

#[test]
fn every_state_draws_at_three_sizes_and_the_buttons_are_hit_regions() {
    let p = Port::new();
    let mut t = mixed();
    for (w, h) in [(1280.0, 720.0), (800.0, 600.0), (1920.0, 1080.0), (420.0, 700.0)] {
        let mut g = Gfx::new(w as u32, h as u32);
        draw(&mut t, &mut g, &p, w, h, 3);
        assert!(g.hits.iter().any(|x| x.ev == Event::Tab(100)), "the Scan button at {w}");
        assert!(g.hits.iter().any(|x| x.ev == Event::Tab(110)), "the Devices button at {w}");
    }
    t.running = true;
    t.progress = (3, 9);
    let mut g = Gfx::new(1280, 720);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 5);
    assert!(g.hits.iter().any(|x| x.ev == Event::Tab(101)), "Stop while running");
    t.running = false;
    t.summary = Some((5, 5, true));
    t.status = Some(("a note".into(), true));
    t.mode = Mode::Form(ScanTile::target_form(), FormKind::Target);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 5);
    t.mode = Mode::Browse;
    t.hosts.clear();
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 5);
    assert!(!t.help().is_empty() && !t.hint().is_empty());
    assert_eq!((t.title(), t.accent()), ("Net scan", ACCENT));
}

#[test]
fn the_service_panel_offers_add_to_models_for_a_model_server_only() {
    let p = Port::new();
    let mut t = scanned(&p);
    let mut g = Gfx::new(1280, 720);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(102)));
    let mut cam = mixed();
    cam.sel = 1;
    let mut g = Gfx::new(1280, 720);
    draw(&mut cam, &mut g, &p, 1280.0, 720.0, 0);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Tab(102)));
}

#[test]
fn a_narrow_machine_list_keeps_the_address_and_a_tooltip_has_all_of_the_row() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    let ip = Ip::new(192, 168, 100, 200);
    let mut h = HostInfo::new(ip);
    h.name = Some("living-room-television-with-a-very-long-friendly-name".into());
    h.mac = Some("aa:bb:cc:dd:ee:ff".into());
    t.hosts.insert(ip, h);
    let mut g = Gfx::new(520, 720);
    draw(&mut t, &mut g, &p, 520.0, 720.0, 5);
    let row = g.hits.iter().find(|h| h.label == "192.168.100.200").expect("the row").rect;
    let head = format!("{ip}  {}", t.hosts[&ip].name.clone().unwrap());
    let cut = fit_keep_ip(&mut g, 16.0, &head, Face::Bold, row.w - 60.0);
    assert!(cut != head && cut.starts_with("192.168.100.200"), "{cut}");
    let plain = g.rgba().to_vec();
    let (mx, my) = ((row.x + 20.0) as i32, (row.y + 10.0) as i32);
    g.mouse = (mx, my);
    t.handle(&Event::MouseMove(mx, my), &p);
    for _ in 0..POINTER_DELAY - 1 {
        t.handle(&Event::Tick, &p);
    }
    draw(&mut t, &mut g, &p, 520.0, 720.0, 5);
    let hovered = g.rgba().to_vec();
    for _ in 0..2 {
        t.handle(&Event::Tick, &p);
    }
    assert!(t.wants_redraw(), "the tooltip appears");
    draw(&mut t, &mut g, &p, 520.0, 720.0, 5);
    assert_ne!(hovered, g.rgba());
    assert_ne!(plain, g.rgba());
    key(&mut t, &p, Key::Down);
    assert!(t.wants_redraw(), "a key press puts it away at once");
}

// ---- copying and adding a model server

#[test]
fn c_copies_the_services_address_or_ip_and_port_and_says_so() {
    let p = Port::new();
    let mut t = scanned(&p);
    key(&mut t, &p, Key::Char('c'));
    assert_eq!(t.status_text(), Some("Copied http://192.168.7.6:11434"));
    t.hosts.get_mut(&Ip::new(192, 168, 7, 6)).unwrap().services[0].url = None;
    key(&mut t, &p, Key::Char('c'));
    assert_eq!(t.status_text(), Some("Copied 192.168.7.6:11434"));
}

#[test]
fn the_add_form_is_named_after_the_machine_and_its_submit_goes_to_the_port_with_the_chosen_model_and_key() {
    let p = Port::new();
    let mut t = scanned(&p);
    key(&mut t, &p, Key::Char('a'));
    let Mode::Form(f, _) = &mut t.mode else { panic!("the add form opens") };
    assert_eq!(f.value(0), "ollama-192-168-7-6");
    assert_eq!(f.value(1), "http://192.168.7.6:11434");
    f.set_value(2, "tok-123");
    f.fields[3].choice = 1;
    key(&mut t, &p, Key::Ctrl('s'));
    assert!(matches!(t.mode, Mode::Browse));
    assert_eq!(t.status_text(), Some("Added"));
    assert_eq!(p.added.lock().unwrap()[0], (Ip::new(192, 168, 7, 6), "ollama-192-168-7-6".into(), "http://192.168.7.6:11434".into(), "qwen3:32b".into(), "tok-123".into()));
}

#[test]
fn a_refused_add_keeps_the_form_open_with_the_reason() {
    let p = Port::new();
    *p.add_result.lock().unwrap() = Err("Only an administrator can add models.".into());
    let mut t = scanned(&p);
    key(&mut t, &p, Key::Char('a'));
    key(&mut t, &p, Key::Ctrl('s'));
    let Mode::Form(f, _) = &t.mode else { panic!("the form stays open") };
    assert_eq!(f.error.as_deref(), Some("Only an administrator can add models."));
}

#[test]
fn a_server_that_lists_no_models_asks_for_the_model_in_a_text_field() {
    let p = Port::new();
    let mut t = ScanTile::new(&p);
    let mut s = Service::new(1234, Kind::LlmServer, "LM Studio");
    s.url = Some("http://192.168.7.9:1234/v1".into());
    let h = host(Ip::new(192, 168, 7, 9), vec![s]);
    t.hosts.insert(h.ip, h);
    key(&mut t, &p, Key::Char('a'));
    let Mode::Form(f, _) = &mut t.mode else { panic!("the add form opens") };
    f.set_value(3, "my-model");
    key(&mut t, &p, Key::Ctrl('s'));
    assert_eq!(p.added.lock().unwrap()[0].3, "my-model");
    assert_eq!(p.added.lock().unwrap()[0].1, "llm-192-168-7-9");
}

#[test]
fn only_a_model_server_and_only_an_administrator_can_add() {
    let p = Port::new();
    let mut t = mixed();
    t.sel = 1; // the camera
    key(&mut t, &p, Key::Char('a'));
    assert!(matches!(t.mode, Mode::Browse));
    assert!(t.status_text().unwrap().starts_with("Only model servers"), "{:?}", t.status_text());
    let mut q = Port::new();
    q.admin = false;
    let mut u = scanned(&q);
    key(&mut u, &q, Key::Char('a'));
    assert!(matches!(u.mode, Mode::Browse));
    assert!(u.status_text().unwrap().contains("administrator"), "{:?}", u.status_text());
    assert!(q.added.lock().unwrap().is_empty());
}

#[test]
fn the_provider_names_and_addresses_follow_the_kind_of_server() {
    let ip = Ip::new(192, 168, 1, 50);
    let ol = Service::new(11434, Kind::Ollama, "Ollama");
    assert_eq!((provider_id(ip, &ol).as_str(), base_url(ip, &ol).as_str()), ("ollama-192-168-1-50", "http://192.168.1.50:11434"));
    let mut lm = Service::new(1234, Kind::LlmServer, "LM Studio");
    assert_eq!(provider_id(ip, &lm), "llm-192-168-1-50");
    assert_eq!(base_url(ip, &lm), "http://192.168.1.50:1234/v1", "an OpenAI-compatible server is reached through /v1");
    lm.port = 8000;
    assert_eq!(provider_id(ip, &lm), "llm-192-168-1-50-8000", "the port is in the name when it is not a default one");
    lm.url = Some("https://box.local:8000/v1".into());
    assert_eq!(base_url(ip, &lm), "https://box.local:8000/v1");
    let lem = Service::new(8000, Kind::LlmServer, "Lemonade Server 1.0");
    assert!(is_lemonade(&lem) && !is_lemonade(&lm) && !is_lemonade(&ollama()));
    assert_eq!(provider_id(ip, &lem), "lemonade-192-168-1-50-8000");
}

// ---- the devices view

fn device(mac: &str, trusted: bool) -> Device {
    Device { mac: mac.into(), ip: "192.168.7.4".into(), iface: "eth0".into(), first_seen: 10_000 - 3_700, last_seen: 10_000 - 65, trusted, name: String::new() }
}

#[test]
fn the_devices_view_lists_the_devices_with_ages_from_the_ports_clock_and_the_button_says_how_many_are_new() {
    let p = Port::new();
    p.watch.devices.lock().unwrap().extend([device("bb:bb:bb:bb:bb:09", false), device("aa:aa:aa:aa:aa:01", true)]);
    let mut t = ScanTile::new(&p);
    let mut g = Gfx::new(1280, 720);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(110)));
    key(&mut t, &p, Key::Char('d'));
    assert!(t.devices);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 0);
    assert_eq!(g.hits.iter().filter(|h| h.label == "device").count(), 2);
    assert_eq!(key(&mut t, &p, Key::Esc), Answer::Stay);
    assert!(!t.devices, "Esc leaves the devices view first");
    assert_eq!(key(&mut t, &p, Key::Esc), Answer::Back);
}

#[test]
fn an_administrator_trusts_names_and_forgets_devices_and_each_change_is_audited_where_it_should_be() {
    let p = Port::new();
    p.watch.devices.lock().unwrap().extend([device("bb:bb:bb:bb:bb:09", false), device("aa:aa:aa:aa:aa:01", true)]);
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('d'));
    key(&mut t, &p, Key::Char('t'));
    assert_eq!(p.watch.trusted.lock().unwrap().as_slice(), ["bb:bb:bb:bb:bb:09"]);
    assert_eq!(t.status_text(), Some("bb:bb:bb:bb:bb:09 is trusted now."));
    key(&mut t, &p, Key::Char('T'));
    assert_eq!(t.status_text(), Some("1 device(s) trusted."));
    key(&mut t, &p, Key::Char('r'));
    type_text(&mut t, &p, "Guest phone");
    key(&mut t, &p, Key::Enter);
    assert_eq!(p.watch.renamed.lock().unwrap().as_slice(), [("bb:bb:bb:bb:bb:09".to_string(), "Guest phone".to_string())]);
    assert_eq!(t.status_text(), Some("Renamed."));
    key(&mut t, &p, Key::Char('x'));
    assert_eq!(p.watch.forgotten.lock().unwrap().as_slice(), ["bb:bb:bb:bb:bb:09"]);
    key(&mut t, &p, Key::Char('w'));
    assert!(*p.watch.on.lock().unwrap());
    key(&mut t, &p, Key::Char('w'));
    assert!(!*p.watch.on.lock().unwrap());
    let log = p.audit_text();
    for line in ["netsec.trust bb:bb:bb:bb:bb:09", "netsec.trust_all 1", "netsec.watch on", "netsec.watch off"] {
        assert!(log.contains(line), "{line} in {log}");
    }
}

#[test]
fn a_device_that_is_no_longer_listed_says_so_when_renamed_and_the_cursor_stays_in_the_list() {
    let p = Port::new();
    p.watch.devices.lock().unwrap().push(device("gone", false));
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('d'));
    key(&mut t, &p, Key::Char('r'));
    key(&mut t, &p, Key::Enter);
    assert_eq!(t.status_text(), Some("That device is no longer listed."));
    for _ in 0..5 {
        key(&mut t, &p, Key::Down);
    }
    assert_eq!(t.dev_sel, 0, "one device: Down stays on it");
    t.handle(&Event::Row(50), &p);
    assert_eq!(t.dev_sel, 0);
}

#[test]
fn an_ordinary_account_can_look_at_the_devices_but_change_nothing() {
    let mut p = Port::new();
    p.admin = false;
    p.watch.devices.lock().unwrap().push(device("aa:aa:aa:aa:aa:01", false));
    let mut t = ScanTile::new(&p);
    key(&mut t, &p, Key::Char('d'));
    for k in ['t', 'T', 'r', 'x', 'w'] {
        key(&mut t, &p, Key::Char(k));
        assert_eq!(t.status_text(), Some("Only administrators can change the device list."), "{k}");
    }
    assert!(p.watch.trusted.lock().unwrap().is_empty() && p.watch.forgotten.lock().unwrap().is_empty() && p.audit.lock().unwrap().is_empty());
    let mut g = Gfx::new(1280, 720);
    draw(&mut t, &mut g, &p, 1280.0, 720.0, 0);
}

#[test]
fn the_mirror_service_keeps_the_better_answer_only_in_the_scanner_and_the_tile_keeps_what_it_is_given() {
    let mut s = Service::new(80, Kind::Web, "nginx");
    assert_eq!((s.proto, s.confidence), ("tcp", Confidence::Medium));
    s.confidence = Confidence::High;
    assert!(Confidence::Low < Confidence::Medium && Confidence::Medium < Confidence::High);
    let h = host(Ip::new(1, 2, 3, 4), vec![s, Service::new(554, Kind::Camera, "Cam")]);
    assert_eq!(h.main_kind(), Kind::Camera, "cameras outrank plain web");
    assert!(h.has(Kind::Web) && !h.risky());
    assert_eq!(HostInfo::new(Ip::new(1, 2, 3, 4)).main_kind(), Kind::Other);
}

#[test]
fn ages_are_written_the_way_the_system_page_writes_them() {
    assert_eq!(human_duration(5), "5s");
    assert_eq!(human_duration(65), "1m 5s");
    assert_eq!(human_duration(3700), "1h 1m");
    assert_eq!(human_duration(90_000), "1d 1h");
}
