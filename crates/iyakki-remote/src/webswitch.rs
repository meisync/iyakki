//! Runs "Serve this desktop to browsers" (`iyakki_agent::webaccess`): while the Settings switch is on, the browser front end listens
//! (HTTPS only, on the chosen port, all addresses) and the firewall allows that TCP port from the local networks
//! (`iyakki_fw::webrule`, listed as "remote access web"); when the switch is off both are gone. Looked at every two seconds, beside
//! the developer-mode and device-name watchers.
//!
//! * A front end started at boot (`iyakki.web`, `--web`, `IYAKKI_WEB`) is never doubled: the page then says "on (set at boot)" and
//!   this watcher leaves that listener alone (and drops its own, should it have raced the boot one).
//! * A person who removes the firewall rule while the switch is on turns the switch off with that (the removal is respected, the
//!   rule is not made again). The change is audited.
//! * A listener that cannot start (port in use) is reported on the page and as "Remote access (web)" in the startup list, audited
//!   once, and tried again every two seconds.

use std::path::Path;
use std::sync::Arc;

use iyakki_agent::startup::Registry;
use iyakki_agent::webaccess::{self, Status};
use iyakki_fw::service::Handle;
use iyakki_fw::webrule;

/// The listener behind a small interface so tests can stand in for the network.
pub trait Listener {
    fn start(&mut self, port: u16) -> Result<(), String>;
    fn stop(&mut self);
}

/// The real one: `Server::start` with the certificate in `<data>/tls`.
pub struct Real {
    env: iyakki_shell::shell::ShellEnv,
    server: Option<crate::server::Server>,
}

impl Real {
    pub fn new(env: iyakki_shell::shell::ShellEnv) -> Real {
        Real { env, server: None }
    }
}

impl Listener for Real {
    fn start(&mut self, port: u16) -> Result<(), String> {
        let dir = self.env.services.dir().to_path_buf();
        let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
        let names: Vec<String> = [host, format!("{}.local", iyakki_agent::devname::label(&dir))].into_iter().filter(|h| h != ".local" && !h.is_empty()).collect();
        let id = crate::tls::identity(&dir.join("tls"), &names)?;
        let addr = format!("0.0.0.0:{port}");
        let s = crate::server::Server::start(crate::server::Config { addr, tls: Some(id.config) }, Arc::new(crate::screen::ShellGate::new(self.env.share()))).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse { format!("port {port} is already in use by another program") } else { format!("cannot listen on port {port}: {e}") }
        })?;
        iyakki_agent::extagent::relay::note_web_front_end(s.local_addr().port(), true);
        self.server = Some(s);
        Ok(())
    }

    fn stop(&mut self) {
        if self.server.take().is_some() {
            iyakki_agent::extagent::relay::clear_web_front_end();
        }
    }
}

/// What is wanted right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub on: bool,
    pub port: u16,
    /// The port of a front end started at boot, if any.
    pub boot: Option<u16>,
}

/// What the tick asks the caller to do besides what it did itself.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The person removed the firewall rule: switch the setting off.
    SwitchOff,
}

pub struct Watcher<L: Listener> {
    l: L,
    running: Option<u16>,
    rule_seen: bool,
    last_error: String,
    last_fw_error: String,
    last_status: Option<Status>,
}

impl<L: Listener> Watcher<L> {
    pub fn new(l: L) -> Watcher<L> {
        Watcher { l, running: None, rule_seen: false, last_error: String::new(), last_fw_error: String::new(), last_status: None }
    }

    pub fn tick(&mut self, want: &Want, dir: &Path, fw: &Handle, reg: &Registry, audit: &dyn Fn(&str, &str)) -> Vec<Event> {
        let mut events = Vec::new();
        let managed = fw.lock().managed();
        if !want.on {
            self.stop();
            if managed {
                self.firewall(fw, None, audit);
            }
            self.rule_seen = false;
            self.last_error.clear();
            self.report(dir, Status::default());
            if want.boot.is_some() { reg.ready(webaccess::SERVICE_ID) } else { reg.stopped(webaccess::SERVICE_ID) };
            return events;
        }
        if managed && self.rule_seen && !webrule::present(&fw.lock()) {
            audit("webaccess.firewall.removed_by_person", "the rule was removed: remote access is switched off");
            self.rule_seen = false;
            events.push(Event::SwitchOff);
            self.stop();
            self.report(dir, Status::default());
            reg.stopped(webaccess::SERVICE_ID);
            return events;
        }
        // a front end of the boot option is the one that serves; the switch does not start a second
        if let Some(p) = want.boot {
            self.stop();
            self.last_error.clear();
            if managed {
                self.firewall(fw, Some(p), audit);
                self.rule_seen = webrule::present(&fw.lock());
            }
            self.report(dir, Status { listening: true, port: p, boot: true, error: String::new() });
            reg.ready(webaccess::SERVICE_ID);
            return events;
        }
        if self.running.is_some_and(|p| p != want.port) {
            audit("webaccess.stop", "the port changed: the listener is closed and opened again");
            self.stop();
        }
        if self.running.is_none() {
            reg.starting(webaccess::SERVICE_ID, "opening the port");
            match self.l.start(want.port) {
                Ok(()) => {
                    self.running = Some(want.port);
                    self.last_error.clear();
                    audit("webaccess.start", &format!("serving this desktop to browsers over HTTPS on port {}", want.port));
                }
                Err(e) => {
                    if self.last_error != e {
                        audit("webaccess.failed", &e);
                        self.last_error = e.clone();
                    }
                    reg.failed(webaccess::SERVICE_ID, &e);
                    self.report(dir, Status { listening: false, port: want.port, boot: false, error: e });
                    return events;
                }
            }
        }
        if managed {
            self.firewall(fw, Some(want.port), audit);
            if webrule::present(&fw.lock()) {
                self.rule_seen = true;
            }
        }
        self.report(dir, Status { listening: true, port: want.port, boot: false, error: String::new() });
        reg.ready(webaccess::SERVICE_ID);
        events
    }

    fn firewall(&mut self, fw: &Handle, port: Option<u16>, audit: &dyn Fn(&str, &str)) {
        match webrule::sync(fw, port, "remote access") {
            Ok(ch) => {
                self.last_fw_error.clear();
                ch.iter().for_each(|(a, d)| audit(a, d));
            }
            Err(e) => {
                if self.last_fw_error != e {
                    audit("webaccess.firewall.failed", &e);
                    self.last_fw_error = e;
                }
            }
        }
    }

    fn stop(&mut self) {
        if self.running.take().is_some() {
            self.l.stop();
        }
    }

    fn report(&mut self, dir: &Path, st: Status) {
        if self.last_status.as_ref() != Some(&st) {
            webaccess::write_status(dir, &st);
            self.last_status = Some(st);
        }
    }
}

/// The thread: every two seconds the switch, the port, the boot front end and the firewall are looked at (see the module text).
pub fn watch(env: iyakki_shell::shell::ShellEnv) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("iyakki-webswitch".into()).spawn(move || {
        let mut w = Watcher::new(Real::new(env.share()));
        // the boot option may start its listener a moment after this thread: look at the switch only after that
        std::thread::sleep(std::time::Duration::from_secs(6));
        loop {
            let want = Want { on: env.services.web_access_on(), port: env.services.web_access_port(), boot: webaccess::boot_port() };
            let reg = iyakki_agent::startup::global();
            let events = w.tick(&want, env.services.dir(), &Handle::Global, reg, &|a, d| env.audit.record("system", a, d));
            for e in events {
                match e {
                    Event::SwitchOff => {
                        let _ = env.services.set_web_access(false);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_fw::backend::FakeBackend;
    use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Store};
    use std::net::IpAddr;
    use std::sync::Mutex;

    struct Clk;
    impl Clock for Clk {
        fn now(&self) -> i64 {
            1_900_000_000
        }
    }
    struct NoDns;
    impl Resolver for NoDns {
        fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            Err("none".into())
        }
    }
    #[derive(Default)]
    struct Mem(Mutex<Option<Saved>>);
    impl Store for Mem {
        fn load(&self) -> Result<Option<Saved>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &Saved) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(s.clone());
            Ok(())
        }
    }

    /// Stands in for the server: the ports in use by "another program", and the port it listens on.
    #[derive(Clone, Default)]
    struct Fake {
        busy: Arc<Mutex<Vec<u16>>>,
        on: Arc<Mutex<Option<u16>>>,
        starts: Arc<Mutex<u32>>,
    }
    impl Listener for Fake {
        fn start(&mut self, port: u16) -> Result<(), String> {
            if self.busy.lock().unwrap().contains(&port) {
                return Err(format!("port {port} is already in use by another program"));
            }
            *self.on.lock().unwrap() = Some(port);
            *self.starts.lock().unwrap() += 1;
            Ok(())
        }
        fn stop(&mut self) {
            *self.on.lock().unwrap() = None;
        }
    }

    fn rig(tag: &str) -> (std::path::PathBuf, Handle, Registry) {
        let dir = std::env::temp_dir().join(format!("iyakki-rwebsw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(Clk), Box::new(NoDns), Box::new(Mem::default()));
        e.boot(false);
        (dir, Handle::Own(Arc::new(Mutex::new(e))), Registry::new(|| 0))
    }

    fn run(w: &mut Watcher<Fake>, want: &Want, dir: &Path, h: &Handle, reg: &Registry) -> (Vec<Event>, Vec<String>) {
        let log = Mutex::new(Vec::new());
        let ev = w.tick(want, dir, h, reg, &|a, d| log.lock().unwrap().push(format!("{a}: {d}")));
        (ev, log.into_inner().unwrap())
    }

    fn rule_ports(h: &Handle) -> Vec<String> {
        h.lock().config().rules.iter().filter(|r| webrule::is_web(r)).map(|r| r.ports.clone()).collect()
    }

    #[test]
    fn the_listener_and_the_rule_exist_only_while_the_switch_is_on_and_follow_the_port() {
        let (dir, h, reg) = rig("flow");
        let fake = Fake::default();
        let mut w = Watcher::new(fake.clone());
        let off = Want { on: false, port: 8443, boot: None };
        let (ev, log) = run(&mut w, &off, &dir, &h, &reg);
        assert!(ev.is_empty() && log.is_empty() && fake.on.lock().unwrap().is_none() && rule_ports(&h).is_empty());
        assert!(!webaccess::read_status(&dir).listening);
        let on = Want { on: true, port: 8443, boot: None };
        let (_, log) = run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(*fake.on.lock().unwrap(), Some(8443));
        assert!(log.iter().any(|l| l.starts_with("webaccess.start: serving this desktop to browsers over HTTPS on port 8443")) && log.iter().any(|l| l.starts_with("webaccess.firewall.allow: TCP 8443")), "{log:?}");
        assert!(rule_ports(&h).iter().all(|p| p == "8443") && !rule_ports(&h).is_empty());
        assert_eq!(reg.state(webaccess::SERVICE_ID), Some(iyakki_agent::startup::StartupState::Ready));
        let st = webaccess::read_status(&dir);
        assert!(st.listening && st.port == 8443 && !st.boot);
        // steady: no second start
        run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(*fake.starts.lock().unwrap(), 1);
        // a new port: closed and opened again, the rule moves
        let (_, log) = run(&mut w, &Want { port: 9443, ..on.clone() }, &dir, &h, &reg);
        assert_eq!(*fake.on.lock().unwrap(), Some(9443));
        assert!(log.iter().any(|l| l.starts_with("webaccess.stop")));
        assert!(rule_ports(&h).iter().all(|p| p == "9443") && !rule_ports(&h).is_empty());
        // off: closed, the rule gone, the status and the startup line say so
        run(&mut w, &off, &dir, &h, &reg);
        assert!(fake.on.lock().unwrap().is_none() && rule_ports(&h).is_empty());
        assert!(!webaccess::read_status(&dir).listening);
        assert_eq!(reg.state(webaccess::SERVICE_ID), Some(iyakki_agent::startup::StartupState::Stopped));
    }

    #[test]
    fn a_front_end_started_at_boot_is_not_doubled() {
        let (dir, h, reg) = rig("boot");
        let fake = Fake::default();
        let mut w = Watcher::new(fake.clone());
        let (_, _) = run(&mut w, &Want { on: true, port: 8443, boot: Some(8443) }, &dir, &h, &reg);
        assert_eq!(*fake.starts.lock().unwrap(), 0, "the boot one serves");
        let st = webaccess::read_status(&dir);
        assert!(st.listening && st.boot && st.port == 8443);
        assert_eq!(st.line(true), "on (set at boot), port 8443");
        // when the watcher already started its own and the boot one turns up, the watcher's is closed
        let mut w2 = Watcher::new(fake.clone());
        run(&mut w2, &Want { on: true, port: 8443, boot: None }, &dir, &h, &reg);
        assert_eq!(*fake.on.lock().unwrap(), Some(8443));
        run(&mut w2, &Want { on: true, port: 8443, boot: Some(8443) }, &dir, &h, &reg);
        assert!(fake.on.lock().unwrap().is_none());
    }

    #[test]
    fn a_port_in_use_is_reported_once_in_the_startup_list_and_the_page_and_tried_again() {
        let (dir, h, reg) = rig("busy");
        let fake = Fake::default();
        fake.busy.lock().unwrap().push(8443);
        let mut w = Watcher::new(fake.clone());
        let on = Want { on: true, port: 8443, boot: None };
        let mut failed = 0;
        for _ in 0..3 {
            let (_, log) = run(&mut w, &on, &dir, &h, &reg);
            failed += log.iter().filter(|l| l.starts_with("webaccess.failed")).count();
        }
        assert_eq!(failed, 1, "audited once");
        assert!(reg.state(webaccess::SERVICE_ID).unwrap().failed());
        let st = webaccess::read_status(&dir);
        assert!(!st.listening && st.error.contains("already in use") && st.line(true).starts_with("not listening"), "{st:?}");
        assert!(rule_ports(&h).is_empty(), "no port is opened in the firewall for a listener that is not there");
        // the other program lets go: the next look succeeds
        fake.busy.lock().unwrap().clear();
        run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(*fake.on.lock().unwrap(), Some(8443));
        assert!(webaccess::read_status(&dir).listening);
    }

    #[test]
    fn a_person_removing_the_rule_switches_it_off_and_it_is_not_made_again() {
        let (dir, h, reg) = rig("removed");
        let fake = Fake::default();
        let mut w = Watcher::new(fake.clone());
        let on = Want { on: true, port: 8443, boot: None };
        run(&mut w, &on, &dir, &h, &reg);
        {
            let mut e = h.lock();
            let mut cfg = e.config().clone();
            cfg.rules.retain(|r| !webrule::is_web(r));
            e.propose(cfg, "admin").unwrap();
            let _ = e.keep("admin");
        }
        let (ev, log) = run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(ev, vec![Event::SwitchOff]);
        assert!(log.iter().any(|l| l.starts_with("webaccess.firewall.removed_by_person")));
        assert!(rule_ports(&h).is_empty() && fake.on.lock().unwrap().is_none());
    }
}
