//! Settings -> Domains: the route list, Add (with the services found in the cluster offered and their port filled in) / Edit / Delete
//! (asks first), the listener, the firewall step and the page drawn at four screen sizes. kubectl, the firewall, the announcer and the
//! vault are fakes; the listener is real, on high loopback ports.

use std::sync::Mutex;

use super::dom_page::DomForm;
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::cloudlisten::{PortGate, ServiceDialer};
use iyakki_agent::domains::{Deps, GateMaker, Kube, NameSink};
use iyakki_agent::gitkeys::MemStore;

struct FakeKube;
impl Kube for FakeKube {
    fn services(&self) -> Result<String, String> {
        Ok(r#"{"items":[{"metadata":{"namespace":"iyakki-store","name":"iyakki-store-nodeport"},"spec":{"type":"NodePort","clusterIP":"10.43.0.11","ports":[{"name":"http","port":8080,"protocol":"TCP","nodePort":30580}]}}]}"#.into())
    }
}
struct NoGates;
impl GateMaker for NoGates {
    fn make(&self, _: iyakki_agent::cloudlisten::gate::GateReach, _: i64) -> Result<Arc<dyn PortGate>, String> {
        Err("the firewall is a fake here".into())
    }
}
#[derive(Default)]
struct Names(Mutex<Vec<String>>);
impl NameSink for Names {
    fn set(&self, n: &[String]) -> Result<(), String> {
        *self.0.lock().unwrap() = n.to_vec();
        Ok(())
    }
}

struct Rig {
    a: SettingsApp,
    cx: AppCx,
    mgr: Arc<iyakki_agent::domains::Manager>,
    names: Arc<Names>,
}

fn rig(tag: &str) -> Rig {
    let cx = cx_with_admin_store(tag);
    let dir = temp_dir(&format!("{tag}-dom"));
    let names = Arc::new(Names::default());
    let deps = Deps {
        kube: Arc::new(FakeKube),
        secrets: Arc::new(MemStore::default()),
        gates: Arc::new(NoGates),
        names: names.clone(),
        notify: Arc::new(|_, _| {}),
        dialer: Arc::new(|_| Err::<Arc<dyn ServiceDialer>, String>("no kubectl in the test".into())),
        held: Arc::new(Vec::new),
        web_port: Arc::new(|| None),
        audit: Arc::new(|_, _| {}),
        now: Arc::new(|| 1_900_000_000),
    };
    let mgr = Arc::new(iyakki_agent::domains::Manager::new(&dir, deps));
    let mut a = SettingsApp::new(&cx);
    a.dom.mgr = Some(mgr.clone());
    for _ in 0..60 {
        if a.tab() == Tab::Domains {
            break;
        }
        a.handle(&Event::Key(Key::Right), &cx);
    }
    assert_eq!(a.tab(), Tab::Domains);
    Rig { a, cx, mgr, names }
}

fn press(r: &mut Rig, k: Key) {
    r.a.handle(&Event::Key(k), &r.cx);
}

fn submit(r: &mut Rig, set: &[(usize, &str)], choices: &[(usize, usize)]) -> Result<String, String> {
    let Some(Modal::Form(FormKind::Domains(k), mut f)) = r.a.modal.take() else { panic!("no domains form is open") };
    for (i, v) in set {
        f.set_value(*i, v);
    }
    for (i, c) in choices {
        f.set_choice(*i, *c);
    }
    r.a.dom_submit(&r.cx, &k, &mut f)
}

fn wait_found(r: &mut Rig) {
    for _ in 0..200 {
        r.a.dom_tick(&r.cx);
        if r.a.dom.found.is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the cluster services never arrived");
}

fn free_port() -> u16 {
    iyakki_agent::testdir::free_port()
}

#[test]
fn a_route_to_a_service_found_in_the_cluster_is_added_with_its_port_filled_in_and_removed_after_asking() {
    let mut r = rig("dom-add");
    wait_found(&mut r);
    press(&mut r, Key::Char('a'));
    let Some(Modal::Form(FormKind::Domains(DomForm::Route(None)), f)) = &r.a.modal else { panic!("no route form") };
    let _ = f;
    // choice 0 is "type the port", then this device's own (the private store), then what the cluster has
    let msg = submit(&mut r, &[(0, "Store.local")], &[(2, 1)]).unwrap();
    assert!(msg.contains("Saved store.local/") && msg.contains("listener is off") && msg.contains(".local"), "{msg}");
    assert_eq!(r.mgr.routes()[0].target, iyakki_agent::cloudroute::Target::Local { port: 30580 });
    // the cluster's own service is the next choice and gives the same port
    press(&mut r, Key::Char('a'));
    submit(&mut r, &[(0, "again.local")], &[(2, 2)]).unwrap();
    assert_eq!(r.mgr.route("again.local", "/").unwrap().target, iyakki_agent::cloudroute::Target::Local { port: 30580 });
    // the engine's words reach the person
    press(&mut r, Key::Char('a'));
    let e = submit(&mut r, &[(0, "not a host"), (3, "80")], &[(2, 0)]).unwrap_err();
    assert!(e.contains("is not a host name"), "{e}");
    press(&mut r, Key::Char('a'));
    let e = submit(&mut r, &[(0, "x.local")], &[(2, 0)]).unwrap_err();
    assert!(e.contains("Type the port"), "{e}");
    // delete asks first
    press(&mut r, Key::Char('d'));
    assert!(matches!(r.a.modal, Some(Modal::Confirm(ConfirmKind::DomainDelete(..), _))));
    r.a.modal = None;
    assert_eq!(r.mgr.routes().len(), 2);
    r.a.dom_delete(&r.cx, "again.local", "/");
    assert_eq!(r.mgr.routes().len(), 1);
}

#[test]
fn the_listener_is_switched_on_and_off_from_the_page_and_the_name_is_announced_while_it_runs() {
    let mut r = rig("dom-listen");
    let up = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    r.mgr.save_route(None, iyakki_agent::cloudroute::Route { host: "store.local".into(), path_prefix: "/".into(), target: iyakki_agent::cloudroute::Target::Local { port: up.local_addr().unwrap().port() }, scheme: iyakki_agent::cloudroute::Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: iyakki_agent::cloudroute::Access::Members, client_ca: None }, None).unwrap();
    press(&mut r, Key::Char('p'));
    let (h, s) = (free_port(), free_port());
    submit(&mut r, &[(0, &h.to_string()), (1, &s.to_string())], &[]).unwrap();
    press(&mut r, Key::Char('x'));
    assert!(r.mgr.status().running, "{:?}", r.mgr.status());
    assert_eq!(*r.names.0.lock().unwrap(), ["store.local"]);
    // the firewall is a separate step, and here the fake firewall refuses: the listener stays off the network
    press(&mut r, Key::Char('f'));
    submit(&mut r, &[], &[(0, 1), (1, 1)]).unwrap();
    assert!(!r.mgr.status().running && r.mgr.status().last_errors.iter().any(|e| e.contains("firewall is not available")), "{:?}", r.mgr.status());
    press(&mut r, Key::Char('x'));
    assert!(!r.mgr.settings().enabled && r.names.0.lock().unwrap().is_empty());
    // the standard ports need the approval choice
    press(&mut r, Key::Char('p'));
    let e = submit(&mut r, &[(0, "80")], &[]).unwrap_err();
    assert!(e.contains("needs your approval"), "{e}");
}

#[test]
fn the_page_is_drawn_at_four_screen_sizes_with_and_without_routes() {
    let mut r = rig("dom-draw");
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = crate::shell::scale_for(w, h);
        r.a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &r.cx, 0);
    }
    r.mgr.save_route(None, iyakki_agent::cloudroute::Route { host: "store.local".into(), path_prefix: "/".into(), target: iyakki_agent::cloudroute::Target::Local { port: 30580 }, scheme: iyakki_agent::cloudroute::Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: iyakki_agent::cloudroute::Access::Members, client_ca: None }, None).unwrap();
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = crate::shell::scale_for(w, h);
        r.a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &r.cx, 0);
    }
}
