//! The MESH tile through the shell's real port and on fake ones: the behaviour tests of the tile (they moved out of `mesh.rs` with the tile
//! crate `iyakki-tile-mesh`, which has its own tests on a fake port) over a real Consul client with a fake upstream.

use super::mesh::*;
use super::mesh_port::MeshPort;
use iyakki_shared::consul::Consul;
use iyakki_tile_mesh::TOKEN_NAME;
use super::{App, AppCx};
use crate::gfx::Gfx;
use crate::ui::{Event, Key, ERR, OK, R};
use std::sync::Arc;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_sec::policy::Role;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<String>>,
    kv: Mutex<BTreeMap<String, String>>,
}

impl Upstream for Fake {
    fn request(&self, method: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        self.asked.lock().unwrap().push(format!("{method} {url}"));
        let path = url.strip_prefix("http://consul.local:8500").ok_or("unreachable")?;
        let ok = |s: &str| Ok(UpstreamResponse { status: 200, body: s.as_bytes().to_vec() });
        let base = path.split('?').next().unwrap_or("");
        match (method, base) {
            ("GET", "/v1/agent/self") => ok(r#"{"Config":{"Version":"1.19.0","Datacenter":"dc1"}}"#),
            ("GET", "/v1/status/leader") => ok("\"10.0.0.1:8300\""),
            ("GET", "/v1/status/peers") => ok(r#"["10.0.0.1:8300"]"#),
            ("GET", "/v1/catalog/services") => ok(r#"{"web":["v1"],"api":[]}"#),
            ("GET", "/v1/health/service/web") => ok(r#"[{"Node":{"Node":"n1","Address":"10.0.0.5"},"Service":{"Address":"","Port":80},"Checks":[{"Name":"http","Status":"critical","Output":"connection refused"}]},{"Node":{"Node":"n2","Address":"10.0.0.6"},"Service":{"Address":"","Port":80},"Checks":[{"Name":"http","Status":"passing","Output":"ok"}]}]"#),
            ("GET", "/v1/catalog/nodes") => ok(r#"[{"Node":"n1","Address":"10.0.0.5"}]"#),
            ("GET", "/v1/kv/") => ok(r#"["app/","motd"]"#),
            ("GET", "/v1/kv/app/") => ok(r#"["app/","app/color"]"#),
            ("GET", p) if p.starts_with("/v1/kv/") => match self.kv.lock().unwrap().get(&p[7..]) {
                Some(v) => Ok(UpstreamResponse { status: 200, body: v.clone().into_bytes() }),
                None => Ok(UpstreamResponse { status: 404, body: vec![] }),
            },
            ("PUT", p) => {
                self.kv.lock().unwrap().insert(p[7..].to_string(), String::from_utf8_lossy(body).into_owned());
                ok("true")
            }
            ("DELETE", p) => {
                self.kv.lock().unwrap().remove(&p[7..]);
                ok("true")
            }
            _ => Ok(UpstreamResponse { status: 404, body: vec![] }),
        }
    }
}

fn cx_with_consul(tag: &str, address: Option<&str>) -> (AppCx, Arc<Fake>) {
    let mut cx = cx_with_admin_store(tag);
    let fake = Arc::new(Fake::default());
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, fake.clone());
    if let Some(a) = address {
        super::mesh_port::save_address(&cx, a).unwrap();
    }
    (cx, fake)
}

fn settle(a: &mut MeshApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        a.handle(&Event::Tick, cx);
        if a.busy == 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("still loading");
}

fn key(a: &mut MeshApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn typ(a: &mut MeshApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}

fn draw(a: &mut MeshApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), cx, 0);
    g
}

#[test]
fn with_no_address_the_app_opens_on_setup_and_an_admin_saves_one() {
    let (cx, fake) = cx_with_consul("mesh1", None);
    let mut a = MeshApp::new(&cx);
    assert_eq!(a.tab(), Tab::Setup);
    key(&mut a, &cx, Key::Char('c'));
    assert!(matches!(a.mode, Mode::Form(..)));
    // a bad address is explained and nothing is saved
    typ(&mut a, &cx, "consul.local");
    key(&mut a, &cx, Key::Ctrl('s'));
    let Mode::Form(f, _) = &a.mode else { panic!("the form stays open") };
    assert!(f.error.as_deref().unwrap().contains("http://"), "{:?}", f.error);
    assert!(load_address(&cx).is_empty());
    // fix it
    let Mode::Form(f, _) = &mut a.mode else { unreachable!() };
    f.set_value(0, "http://consul.local:8500");
    key(&mut a, &cx, Key::Ctrl('s'));
    settle(&mut a, &cx);
    assert_eq!(load_address(&cx), "http://consul.local:8500");
    assert_eq!(a.cluster.as_ref().unwrap().datacenter, "dc1", "the connection was checked");
    assert!(fake.asked.lock().unwrap().iter().any(|r| r.ends_with("/v1/agent/self")));
    assert!(std::fs::read_to_string(cx.audit.path_buf()).unwrap().contains("mesh.config"));
}

#[test]
fn services_show_their_instances_with_health() {
    let (cx, _) = cx_with_consul("mesh2", Some("http://consul.local:8500"));
    let mut a = MeshApp::new(&cx);
    assert_eq!(a.tab(), Tab::Services);
    settle(&mut a, &cx);
    assert_eq!(a.service_names(), vec!["api", "web"]);
    assert!(a.cluster.as_ref().unwrap().leader.contains("8300"));
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    let rows = a.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].title.contains("n1") && rows[0].sub.contains("critical") && rows[0].sub.contains("connection refused"), "{}", rows[0].sub);
    assert_eq!((rows[0].color, rows[1].color), (ERR, OK));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label.contains("n1")));
    key(&mut a, &cx, Key::Esc);
    assert!(matches!(a.mode, Mode::Browse) && a.tab() == Tab::Services);
    a.handle(&Event::Tab(1), &cx);
    settle(&mut a, &cx);
    assert_eq!(a.rows()[0].title, "n1");
}

#[test]
fn the_key_value_store_is_browsed_changed_and_audited() {
    let (cx, fake) = cx_with_consul("mesh3", Some("http://consul.local:8500"));
    fake.kv.lock().unwrap().insert("app/color".into(), "blue".into());
    let mut a = MeshApp::new(&cx);
    settle(&mut a, &cx);
    a.handle(&Event::Tab(2), &cx);
    settle(&mut a, &cx);
    assert_eq!(a.key_names(), ["app/", "motd"]);
    key(&mut a, &cx, Key::Enter); // into app/
    settle(&mut a, &cx);
    assert_eq!(a.key_names(), ["app/color"]);
    key(&mut a, &cx, Key::Enter); // open the value
    settle(&mut a, &cx);
    assert!(matches!(&a.mode, Mode::Value(k, v) if k == "app/color" && v == "blue"));
    // change it
    key(&mut a, &cx, Key::Char('e'));
    for _ in 0..4 {
        key(&mut a, &cx, Key::Backspace);
    }
    typ(&mut a, &cx, "green");
    key(&mut a, &cx, Key::Ctrl('s'));
    settle(&mut a, &cx);
    assert_eq!(fake.kv.lock().unwrap().get("app/color").map(String::as_str), Some("green"));
    assert!(a.status_text().unwrap().contains("Saved app/color"));
    // a new key
    key(&mut a, &cx, Key::Char('n'));
    typ(&mut a, &cx, "size");
    key(&mut a, &cx, Key::Enter);
    typ(&mut a, &cx, "big");
    key(&mut a, &cx, Key::Ctrl('s'));
    settle(&mut a, &cx);
    assert_eq!(fake.kv.lock().unwrap().get("app/size").map(String::as_str), Some("big"), "{:?}", fake.kv.lock().unwrap());
    // delete asks first
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(matches!(&a.mode, Mode::Value(..)));
    key(&mut a, &cx, Key::Char('d'));
    assert!(matches!(a.mode, Mode::ConfirmDelete(_)));
    key(&mut a, &cx, Key::Char('x'));
    assert!(!fake.kv.lock().unwrap().is_empty() && matches!(a.mode, Mode::Browse), "anything but y cancels");
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::Char('d'));
    key(&mut a, &cx, Key::Char('y'));
    settle(&mut a, &cx);
    assert_eq!(fake.kv.lock().unwrap().len(), 1, "one key deleted");
    let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
    for what in ["mesh.kv.get", "mesh.kv.put", "mesh.kv.delete"] {
        assert!(log.contains(what), "{what} in {log}");
    }
    // up a folder with Backspace
    a.mode = Mode::Browse;
    key(&mut a, &cx, Key::Backspace);
    assert_eq!(a.prefix, "");
}

#[test]
fn the_hosts_an_account_may_reach_decide_and_only_admins_configure() {
    let (mut cx, fake) = cx_with_consul("mesh4", Some("http://consul.local:8500"));
    cx.session.role = Role::Operator;
    cx.session.policy = Role::Operator.template();
    cx.session.policy.net_hosts = vec!["other.example".into()];
    let mut a = MeshApp::new(&cx);
    settle(&mut a, &cx);
    assert!(a.status_text().unwrap().contains("may not reach consul.local"), "{:?}", a.status_text());
    assert!(fake.asked.lock().unwrap().is_empty(), "nothing was sent");
    a.handle(&Event::Tab(3), &cx);
    key(&mut a, &cx, Key::Char('c'));
    assert!(matches!(a.mode, Mode::Browse), "operators cannot open the connection form");
    assert!(a.status_text().unwrap().contains("administrator"));
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.label.starts_with("Change")), "no Change button for them");
}

#[test]
fn every_tab_and_mode_draws_at_several_sizes() {
    let (cx, fake) = cx_with_consul("mesh5", Some("http://consul.local:8500"));
    fake.kv.lock().unwrap().insert("app/color".into(), "blue\nline two".into());
    let mut a = MeshApp::new(&cx);
    settle(&mut a, &cx);
    for (w, h) in [(1280, 720), (800, 600), (1920, 1080)] {
        for i in 0..4 {
            a.handle(&Event::Tab(i), &cx);
            settle(&mut a, &cx);
            let _ = draw(&mut a, &cx, w, h);
        }
        a.mode = Mode::Value("app/color".into(), "blue\nline two".into());
        let _ = draw(&mut a, &cx, w, h);
        a.mode = Mode::ConfirmDelete("app/color".into());
        let _ = draw(&mut a, &cx, w, h);
        a.mode = Mode::Form(a.config_form(), FormKind::Config);
        let _ = draw(&mut a, &cx, w, h);
        a.mode = Mode::Browse;
    }
    assert!(!a.help().is_empty());
}

// ---- the tile on a fake port: no shell context, no vault, no real network ------------------------------------------------------------------

struct FakePort {
    admin: bool,
    allowed: bool,
    address: Mutex<String>,
    tokens: Mutex<Vec<(String, String)>>,
    token_fails: bool,
    audit: Mutex<Vec<(String, String)>>,
    upstream: Arc<Fake>,
    clients: Mutex<usize>,
}

impl FakePort {
    fn new(address: &str) -> FakePort {
        FakePort { admin: true, allowed: true, address: Mutex::new(address.into()), tokens: Mutex::new(vec![]), token_fails: false, audit: Mutex::new(vec![]), upstream: Arc::new(Fake::default()), clients: Mutex::new(0) }
    }
    fn audit_text(&self) -> String {
        self.audit.lock().unwrap().iter().map(|(a, s)| format!("{a} {s}")).collect::<Vec<_>>().join("\n")
    }
}

impl MeshPort for FakePort {
    fn load_address(&self) -> String {
        self.address.lock().unwrap().clone()
    }
    fn save_address(&self, address: &str) -> Result<(), String> {
        *self.address.lock().unwrap() = address.into();
        Ok(())
    }
    fn can_configure(&self) -> bool {
        self.admin
    }
    fn allows_host(&self, _host: &str) -> bool {
        self.allowed
    }
    fn consul(&self, address: &str, _token_name: &str) -> Result<Consul, String> {
        *self.clients.lock().unwrap() += 1;
        Consul::new(address, None, self.upstream.clone())
    }
    fn store_token(&self, token_name: &str, token: &str) -> Result<(), String> {
        if self.token_fails {
            return Err("the vault is locked".into());
        }
        self.tokens.lock().unwrap().push((token_name.into(), token.into()));
        Ok(())
    }
    fn has_token(&self, token_name: &str) -> bool {
        self.tokens.lock().unwrap().iter().any(|(n, _)| n == token_name)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.lock().unwrap().push((action.into(), subject.into()));
    }
}

/// Wait for every answer the tile asked for, without a clock: block on its own channel, hand each answer back and let it read them.
fn await_all(a: &mut MeshApp, port: &dyn MeshPort) {
    while a.busy > 0 {
        let m = a.rx.recv().expect("an answer");
        a.tx.send(m).unwrap();
        a.poll(port);
    }
}

#[test]
fn fake_port_with_no_address_the_tile_opens_on_setup_and_asks_no_client() {
    let port = FakePort::new("");
    let a = MeshApp::with_port(&port);
    assert_eq!(a.tab(), Tab::Setup);
    assert_eq!(*port.clients.lock().unwrap(), 0);
}

#[test]
fn fake_port_an_administrator_saves_the_address_and_token_through_the_port_and_it_is_audited() {
    let port = FakePort::new("");
    let mut a = MeshApp::with_port(&port);
    let mut f = a.config_form();
    f.set_value(0, "http://consul.local:8500");
    f.set_value(1, "s3cret-token");
    a.submit(&f, FormKind::Config, &port);
    assert_eq!(port.load_address(), "http://consul.local:8500");
    assert_eq!(port.tokens.lock().unwrap().as_slice(), [(TOKEN_NAME.to_string(), "s3cret-token".to_string())]);
    assert_eq!(port.audit_text(), "mesh.config consul.local");
    assert!(!port.audit_text().contains("s3cret"), "the token never reaches the audit log");
    await_all(&mut a, &port);
    assert!(port.upstream.asked.lock().unwrap().iter().any(|r| r.ends_with("/v1/agent/self")), "the connection is checked");
    assert!(a.cluster.is_some());
}

#[test]
fn fake_port_an_ordinary_account_cannot_change_the_connection() {
    let mut port = FakePort::new("");
    port.admin = false;
    let mut a = MeshApp::with_port(&port);
    let mut f = a.config_form();
    f.set_value(0, "http://consul.local:8500");
    a.submit(&f, FormKind::Config, &port);
    assert_eq!(port.load_address(), "");
    assert_eq!(a.status_text(), Some("Only an administrator can change the connection."));
    assert!(port.audit.lock().unwrap().is_empty() && port.tokens.lock().unwrap().is_empty());
}

#[test]
fn fake_port_a_token_the_vault_refuses_stops_the_save_and_says_why() {
    let mut port = FakePort::new("");
    port.token_fails = true;
    let mut a = MeshApp::with_port(&port);
    let mut f = a.config_form();
    f.set_value(0, "http://consul.local:8500");
    f.set_value(1, "tok");
    a.submit(&f, FormKind::Config, &port);
    assert_eq!(port.load_address(), "", "the address is not kept when the token was not");
    assert!(a.status_text().unwrap().contains("The token was not saved: the vault is locked"));
}

#[test]
fn fake_port_a_host_the_account_may_not_reach_is_refused_before_any_client_is_made() {
    let mut port = FakePort::new("http://consul.local:8500");
    port.allowed = false;
    let a = MeshApp::with_port(&port);
    assert_eq!(a.status_text(), Some("Your account may not reach consul.local."));
    assert_eq!(*port.clients.lock().unwrap(), 0);
    assert_eq!(a.busy, 0);
}

#[test]
fn fake_port_a_key_put_is_audited_before_it_is_sent_and_lands_in_the_store() {
    let port = FakePort::new("http://consul.local:8500");
    let mut a = MeshApp::with_port(&port);
    await_all(&mut a, &port);
    let mut f = a.new_key_form();
    f.set_value(0, "app/size");
    f.set_value(1, "big");
    a.submit(&f, FormKind::NewKey, &port);
    assert_eq!(port.audit_text(), "mesh.kv.put app/size");
    await_all(&mut a, &port);
    assert_eq!(port.upstream.kv.lock().unwrap().get("app/size").map(String::as_str), Some("big"));
    assert!(a.status_text().unwrap().contains("Saved app/size"));
}
