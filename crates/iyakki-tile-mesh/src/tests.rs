//! The MESH tile on a fake port and a fake Consul client: no shell context, no vault, no network. Answers are awaited by blocking on the
//! tile's own channel (no clock, no sleeps).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_tile_api::mesh::{Check, ClusterInfo, Health, Instance, MeshClient, NodeInfo, ServiceInfo};
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

#[derive(Default)]
struct Cluster {
    kv: Mutex<BTreeMap<String, String>>,
    asked: Mutex<Vec<String>>,
    fail: Mutex<Option<String>>,
}

impl Cluster {
    fn note(&self, what: &str) -> Result<(), String> {
        self.asked.lock().unwrap().push(what.to_string());
        match self.fail.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl MeshClient for Cluster {
    fn cluster(&self) -> Result<ClusterInfo, String> {
        self.note("cluster")?;
        Ok(ClusterInfo { leader: "10.0.0.1:8300".into(), peers: vec!["10.0.0.1:8300".into()], version: "1.19.0".into(), datacenter: "dc1".into() })
    }
    fn services(&self) -> Result<Vec<ServiceInfo>, String> {
        self.note("services")?;
        Ok(vec![ServiceInfo { name: "web".into(), tags: vec!["v1".into()] }, ServiceInfo { name: "api".into(), tags: vec![] }])
    }
    fn instances(&self, service: &str) -> Result<Vec<Instance>, String> {
        self.note(&format!("instances {service}"))?;
        let bad = Check { name: "http".into(), status: Health::Critical, output: "connection refused".into() };
        let good = Check { name: "http".into(), status: Health::Passing, output: "ok".into() };
        Ok(vec![
            Instance { node: "n1".into(), address: "10.0.0.5".into(), port: 80, status: Health::Critical, checks: vec![bad] },
            Instance { node: "n2".into(), address: "10.0.0.6".into(), port: 80, status: Health::Passing, checks: vec![good] },
        ])
    }
    fn nodes(&self) -> Result<Vec<NodeInfo>, String> {
        self.note("nodes")?;
        Ok(vec![NodeInfo { name: "n1".into(), address: "10.0.0.5".into() }])
    }
    fn kv_list(&self, prefix: &str) -> Result<Vec<String>, String> {
        self.note(&format!("kv_list {prefix}"))?;
        Ok(if prefix.is_empty() { vec!["app/".into(), "motd".into()] } else { vec!["app/".into(), "app/color".into()] })
    }
    fn kv_get(&self, key: &str) -> Result<Option<String>, String> {
        self.note(&format!("kv_get {key}"))?;
        Ok(self.kv.lock().unwrap().get(key).cloned())
    }
    fn kv_put(&self, key: &str, value: &str) -> Result<(), String> {
        self.note(&format!("kv_put {key}"))?;
        self.kv.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn kv_delete(&self, key: &str) -> Result<(), String> {
        self.note(&format!("kv_delete {key}"))?;
        self.kv.lock().unwrap().remove(key);
        Ok(())
    }
}

struct Fake {
    admin: bool,
    allowed: bool,
    address: Mutex<String>,
    tokens: Mutex<Vec<(String, String)>>,
    token_fails: bool,
    audits: Mutex<Vec<String>>,
    cluster: Arc<Cluster>,
    clients: Mutex<usize>,
}

impl Fake {
    fn new(address: &str) -> Fake {
        Fake { admin: true, allowed: true, address: Mutex::new(address.into()), tokens: Mutex::new(vec![]), token_fails: false, audits: Mutex::new(vec![]), cluster: Arc::new(Cluster::default()), clients: Mutex::new(0) }
    }
    fn audit_text(&self) -> String {
        self.audits.lock().unwrap().join("\n")
    }
}

impl MeshPort for Fake {
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
    fn client(&self, _address: &str, _token_name: &str) -> Result<Arc<dyn MeshClient>, String> {
        *self.clients.lock().unwrap() += 1;
        Ok(self.cluster.clone())
    }
    fn valid_address(&self, address: &str) -> Result<String, String> {
        let a = address.trim().trim_end_matches('/');
        if a.starts_with("http://") || a.starts_with("https://") {
            Ok(a.to_string())
        } else {
            Err("the address starts with http:// or https://".into())
        }
    }
    fn host_of(&self, address: &str) -> String {
        address.split("://").nth(1).unwrap_or(address).split([':', '/']).next().unwrap_or("").to_string()
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
        self.audits.lock().unwrap().push(format!("{action} {subject}"));
    }
}

/// Wait for every answer the tile asked for: block on its own channel, hand each answer back and let it read them.
fn await_all(t: &mut MeshTile, port: &dyn MeshPort) {
    while t.busy > 0 {
        let m = t.rx.recv().expect("an answer");
        t.tx.send(m).unwrap();
        t.poll(port);
    }
}

fn press(t: &mut MeshTile, p: &Fake, k: Key) {
    t.handle(&Event::Key(k), p);
}

fn draw(t: &mut MeshTile, p: &Fake, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    t.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), p, 0);
    g
}

fn text(g: &Gfx) -> String {
    g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn with_no_address_the_tile_opens_on_setup_and_asks_for_no_client() {
    let p = Fake::new("");
    let t = MeshTile::new(&p);
    assert_eq!(t.tab(), Tab::Setup);
    assert_eq!(*p.clients.lock().unwrap(), 0);
    assert_eq!((t.title(), t.accent()), ("Mesh", ACCENT));
}

#[test]
fn with_an_address_the_tile_opens_on_services_and_loads_them_and_the_cluster() {
    let p = Fake::new("http://consul.local:8500");
    let mut t = MeshTile::new(&p);
    assert_eq!(t.tab(), Tab::Services);
    await_all(&mut t, &p);
    assert_eq!(t.service_names(), ["web", "api"]);
    assert_eq!(t.cluster.as_ref().map(|c| c.datacenter.as_str()), Some("dc1"));
    let g = draw(&mut t, &p, 1280, 720);
    let tx = text(&g);
    assert!(tx.contains("web") && tx.contains("v1") && tx.contains("no tags"), "{tx}");
}

#[test]
fn an_instance_list_shows_health_and_the_failing_check_in_words() {
    let p = Fake::new("http://consul.local:8500");
    let mut t = MeshTile::new(&p);
    await_all(&mut t, &p);
    press(&mut t, &p, Key::Enter);
    assert!(matches!(&t.mode, Mode::Instances(n) if n == "web"));
    await_all(&mut t, &p);
    let rows = t.rows();
    assert_eq!((rows[0].color, rows[1].color), (ERR, OK));
    assert!(rows[0].sub.contains("critical") && rows[0].sub.contains("connection refused"), "{}", rows[0].sub);
    assert!(rows[1].sub.contains("passing") && rows[1].sub.contains("1 check(s) ok"));
    press(&mut t, &p, Key::Esc);
    assert!(matches!(t.mode, Mode::Browse) && t.tab() == Tab::Services, "Esc leaves the instances, not the tile");
}

#[test]
fn the_tabs_switch_by_number_and_arrow_keys_and_load_their_own_data() {
    let p = Fake::new("http://consul.local:8500");
    let mut t = MeshTile::new(&p);
    await_all(&mut t, &p);
    press(&mut t, &p, Key::Char('2'));
    await_all(&mut t, &p);
    assert_eq!((t.tab(), t.rows()[0].title.as_str()), (Tab::Nodes, "n1"));
    press(&mut t, &p, Key::Right);
    await_all(&mut t, &p);
    assert_eq!(t.tab(), Tab::Kv);
    assert_eq!(t.key_names(), ["app/", "motd"]);
    press(&mut t, &p, Key::Left);
    assert_eq!(t.tab(), Tab::Nodes);
    t.handle(&Event::Tab(3), &p);
    assert_eq!(t.tab(), Tab::Setup);
}

#[test]
fn the_key_value_store_is_browsed_into_a_folder_read_changed_and_deleted_with_every_step_audited() {
    let p = Fake::new("http://consul.local:8500");
    p.cluster.kv.lock().unwrap().insert("app/color".into(), "blue".into());
    let mut t = MeshTile::new(&p);
    press(&mut t, &p, Key::Char('3'));
    await_all(&mut t, &p);
    press(&mut t, &p, Key::Enter);
    await_all(&mut t, &p);
    assert_eq!(t.prefix, "app/");
    t.sel = 1;
    press(&mut t, &p, Key::Enter);
    await_all(&mut t, &p);
    assert!(matches!(&t.mode, Mode::Value(k, v) if k == "app/color" && v == "blue"));
    assert!(p.audit_text().contains("mesh.kv.get app/color"));
    press(&mut t, &p, Key::Esc);
    press(&mut t, &p, Key::Backspace);
    assert_eq!(t.prefix, "", "Backspace goes up a folder");
    // a new key: audited before it is sent
    let mut f = t.new_key_form();
    f.set_value(0, "app/size");
    f.set_value(1, "large");
    t.submit(&f, FormKind::NewKey, &p);
    let pos = |what: &str| p.cluster.asked.lock().unwrap().iter().position(|a| a == what);
    assert!(p.audit_text().contains("mesh.kv.put app/size"));
    await_all(&mut t, &p);
    assert_eq!(p.cluster.kv.lock().unwrap().get("app/size").map(String::as_str), Some("large"));
    assert!(pos("kv_put app/size").is_some());
    assert_eq!(t.status_text(), Some("Saved app/size"));
    // delete asks first; anything but y cancels
    t.mode = Mode::ConfirmDelete("app/size".into());
    press(&mut t, &p, Key::Char('n'));
    assert!(p.cluster.kv.lock().unwrap().contains_key("app/size") && matches!(t.mode, Mode::Browse));
    t.mode = Mode::ConfirmDelete("app/size".into());
    press(&mut t, &p, Key::Char('y'));
    await_all(&mut t, &p);
    assert!(!p.cluster.kv.lock().unwrap().contains_key("app/size"));
    assert!(p.audit_text().contains("mesh.kv.delete app/size"));
}

#[test]
fn an_administrator_saves_the_address_and_token_and_the_token_never_reaches_the_audit_line() {
    let p = Fake::new("");
    let mut t = MeshTile::new(&p);
    let mut f = t.config_form();
    f.set_value(0, "http://consul.local:8500");
    f.set_value(1, "s3cret-token");
    t.submit(&f, FormKind::Config, &p);
    assert_eq!(p.load_address(), "http://consul.local:8500");
    assert_eq!(p.tokens.lock().unwrap().as_slice(), [(TOKEN_NAME.to_string(), "s3cret-token".to_string())]);
    assert_eq!(p.audit_text(), "mesh.config consul.local");
    await_all(&mut t, &p);
    assert!(t.cluster.is_some(), "the connection is checked once saved");
}

#[test]
fn a_bad_address_is_explained_in_the_form_and_nothing_is_saved() {
    let p = Fake::new("");
    let mut t = MeshTile::new(&p);
    press(&mut t, &p, Key::Char('c'));
    assert!(matches!(t.mode, Mode::Form(..)));
    for c in "consul.local".chars() {
        press(&mut t, &p, Key::Char(c));
    }
    press(&mut t, &p, Key::Ctrl('s'));
    let Mode::Form(f, _) = &t.mode else { panic!("the form stays open") };
    assert!(f.error.as_deref().unwrap().contains("http://"), "{:?}", f.error);
    assert!(p.load_address().is_empty() && p.audit_text().is_empty());
}

#[test]
fn an_ordinary_account_cannot_open_or_submit_the_connection_form() {
    let mut p = Fake::new("");
    p.admin = false;
    let mut t = MeshTile::new(&p);
    press(&mut t, &p, Key::Char('c'));
    assert!(matches!(t.mode, Mode::Browse));
    let mut f = t.config_form();
    f.set_value(0, "http://consul.local:8500");
    t.submit(&f, FormKind::Config, &p);
    assert_eq!(t.status_text(), Some("Only an administrator can change the connection."));
    assert!(p.load_address().is_empty() && p.tokens.lock().unwrap().is_empty());
}

#[test]
fn a_token_the_vault_refuses_stops_the_save_and_says_why() {
    let mut p = Fake::new("");
    p.token_fails = true;
    let mut t = MeshTile::new(&p);
    let mut f = t.config_form();
    f.set_value(0, "http://consul.local:8500");
    f.set_value(1, "x");
    t.submit(&f, FormKind::Config, &p);
    assert!(t.status_text().unwrap().contains("the vault is locked"));
    assert!(p.load_address().is_empty(), "no address is saved without its token");
}

#[test]
fn a_host_the_account_may_not_reach_is_refused_before_any_client_is_made() {
    let mut p = Fake::new("http://consul.local:8500");
    p.allowed = false;
    let t = MeshTile::new(&p);
    assert_eq!(t.status_text(), Some("Your account may not reach consul.local."));
    assert_eq!(*p.clients.lock().unwrap(), 0);
}

#[test]
fn a_cluster_that_answers_with_an_error_is_shown_as_the_status() {
    let p = Fake::new("http://consul.local:8500");
    *p.cluster.fail.lock().unwrap() = Some("connection refused".into());
    let mut t = MeshTile::new(&p);
    await_all(&mut t, &p);
    assert_eq!(t.status_text(), Some("connection refused"));
    assert!(t.services.is_empty());
}

#[test]
fn the_test_key_checks_the_connection_and_audits_the_host_not_the_token() {
    let p = Fake::new("http://consul.local:8500");
    let mut t = MeshTile::new(&p);
    await_all(&mut t, &p);
    press(&mut t, &p, Key::Char('4'));
    await_all(&mut t, &p);
    press(&mut t, &p, Key::Char('t'));
    await_all(&mut t, &p);
    assert!(p.audit_text().contains("mesh.test consul.local"), "{}", p.audit_text());
}

#[test]
fn every_tab_and_mode_draws_inside_its_area_at_several_sizes() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (2560, 1600)] {
        let p = Fake::new("http://consul.local:8500");
        let mut t = MeshTile::new(&p);
        await_all(&mut t, &p);
        for tab in [Tab::Services, Tab::Nodes, Tab::Kv, Tab::Setup] {
            t.set_tab(tab, &p);
            await_all(&mut t, &p);
            for mode in [Mode::Browse, Mode::Value("app/color".into(), "blue\nline two".into()), Mode::ConfirmDelete("app/color".into())] {
                t.mode = mode;
                let g = draw(&mut t, &p, w, h);
                assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h} {tab:?}: nothing drawn");
                let a = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
                for hit in &g.hits {
                    let r = hit.rect;
                    assert!(r.x >= a.x - 0.5 && r.x + r.w <= a.x + a.w + 0.5, "{w}x{h} {tab:?}: {:?} {r:?} leaves the area", hit.label);
                }
            }
            t.mode = Mode::Browse;
        }
    }
}
