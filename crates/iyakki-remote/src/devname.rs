//! Runs the stable device name (`iyakki_agent::devname`, `iyakki_netscan::mdns`): while "Announce this device on the local network" is
//! on, the announcer thread runs and the firewall allows UDP 5353 from the local networks (`iyakki_fw::namerule`, listed as "device
//! name (mDNS)"); when it is off both are gone. Looked at every two seconds, beside the developer-mode watcher.
//!
//! * A person who removes the firewall rule while the switch is on turns the switch off with that (the removal is respected, the rule
//!   is not made again). The change is audited.
//! * The developer service is announced only while developer mode is on, on its port.
//! * The startup registry line "Device name (mDNS)" says starting (probing), ready, failed (why) or stopped.

use std::path::Path;

use iyakki_agent::devname::{self, mdns, Status};
use iyakki_agent::startup::Registry;
use iyakki_fw::namerule;
use iyakki_fw::service::Handle;
use mdns::responder::Svc;

/// The announcer behind a small interface so tests can stand in for the network.
pub trait Net {
    fn start(&mut self, label: &str, svcs: Vec<Svc>) -> Result<(), String>;
    fn set_services(&mut self, svcs: Vec<Svc>);
    /// Extra `.local` names for this device's addresses (domain routes, `iyakki_agent::domains`).
    fn set_extra_names(&mut self, names: Vec<String>);
    fn stop(&mut self);
    fn status(&self) -> Option<mdns::net::Status>;
}

#[derive(Default)]
pub struct Real(Option<mdns::net::Announcer>);

impl Net for Real {
    fn start(&mut self, label: &str, svcs: Vec<Svc>) -> Result<(), String> {
        self.0 = Some(mdns::net::Announcer::start(label, svcs)?);
        Ok(())
    }
    fn set_services(&mut self, svcs: Vec<Svc>) {
        if let Some(a) = &self.0 {
            a.set_services(svcs);
        }
    }
    fn set_extra_names(&mut self, names: Vec<String>) {
        if let Some(a) = &self.0 {
            a.set_extra_names(names);
        }
    }
    fn stop(&mut self) {
        if let Some(mut a) = self.0.take() {
            a.stop();
        }
    }
    fn status(&self) -> Option<mdns::net::Status> {
        self.0.as_ref().map(|a| a.status())
    }
}

/// What is wanted right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub on: bool,
    pub dev_on: bool,
    pub dev_port: u16,
    pub web_port: Option<u16>,
    /// Names of domain routes (`store.local`) while the domain listener runs: announced as more names for this device.
    pub extra_names: Vec<String>,
}

pub struct Watcher<N: Net> {
    net: N,
    running: bool,
    rule_seen: bool,
    last_error: String,
    last_status: Option<Status>,
}

/// What the tick asks the caller to do besides what it did itself.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The person removed the firewall rule: switch the setting off.
    SwitchOff,
}

impl<N: Net> Watcher<N> {
    pub fn new(net: N) -> Watcher<N> {
        Watcher { net, running: false, rule_seen: false, last_error: String::new(), last_status: None }
    }

    pub fn tick(&mut self, want: &Want, dir: &Path, fw: &Handle, reg: &Registry, audit: &dyn Fn(&str, &str)) -> Vec<Event> {
        let mut events = Vec::new();
        let managed = fw.lock().managed();
        if want.on && managed {
            if self.rule_seen && !namerule::present(&fw.lock()) {
                audit("devname.firewall.removed_by_person", "the rule was removed: announcing is switched off");
                self.rule_seen = false;
                events.push(Event::SwitchOff);
                self.stop(dir, reg);
                return events;
            }
            match namerule::sync(fw, true, "device name") {
                Ok(ch) => ch.iter().for_each(|(a, d)| audit(a, d)),
                Err(e) => self.fail(&format!("firewall: {e}"), audit, reg),
            }
            if namerule::present(&fw.lock()) {
                self.rule_seen = true;
            }
        }
        if !want.on {
            if managed {
                if let Ok(ch) = namerule::sync(fw, false, "device name") {
                    ch.iter().for_each(|(a, d)| audit(a, d));
                }
            }
            self.rule_seen = false;
            self.stop(dir, reg);
            return events;
        }
        let svcs = devname::services(want.dev_on, want.dev_port, want.web_port);
        if !self.running {
            match self.net.start(&devname::label(dir), svcs) {
                Ok(()) => {
                    self.running = true;
                    self.last_error.clear();
                    reg.starting("devname", "checking that the name is free");
                    audit("devname.announce.start", "announcing the device name on the local network");
                }
                Err(e) => {
                    self.fail(&e, audit, reg);
                    return events;
                }
            }
        } else {
            self.net.set_services(svcs);
        }
        self.net.set_extra_names(want.extra_names.clone());
        if let Some(s) = self.net.status() {
            let st = Status {
                on: true,
                host: s.host.clone(),
                probing: s.probing,
                addresses: s.v4.iter().map(|a| a.to_string()).chain(s.v6.iter().map(|a| a.to_string())).collect(),
                services: s.services.clone(),
                note: s.error.clone(),
            };
            if s.probing || st.addresses.is_empty() {
                reg.starting("devname", if st.addresses.is_empty() { "waiting for a private network" } else { "checking that the name is free" });
            } else {
                reg.ready("devname");
            }
            if self.last_status.as_ref() != Some(&st) {
                devname::write_status(dir, &st);
                self.last_status = Some(st);
            }
        }
        events
    }

    fn fail(&mut self, e: &str, audit: &dyn Fn(&str, &str), reg: &Registry) {
        if self.last_error != e {
            audit("devname.announce.failed", e);
            self.last_error = e.to_string();
        }
        reg.failed("devname", e);
    }

    fn stop(&mut self, dir: &Path, reg: &Registry) {
        if self.running {
            self.net.stop();
            self.running = false;
            self.last_status = None;
        }
        if self.last_status.is_none() {
            devname::write_status(dir, &Status::default());
        }
        reg.stopped("devname");
    }
}

/// The thread: every two seconds the switch, developer mode and the firewall are looked at (see the module text).
pub fn watch(env: iyakki_shell::shell::ShellEnv) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("iyakki-devname".into()).spawn(move || {
        let dm = iyakki_agent::devmode::DevMode::open(env.services.dir());
        let mut w = Watcher::new(Real::default());
        loop {
            let want = Want { on: env.services.announce_name_on(), dev_on: dm.is_on(), dev_port: dm.state().port(), web_port: Some(iyakki_agent::webaccess::read_status(env.services.dir())).filter(|s| s.listening).map(|s| s.port), extra_names: iyakki_agent::domains::read_names(env.services.dir()) };
            let reg = iyakki_agent::startup::global();
            let events = w.tick(&want, env.services.dir(), &Handle::Global, reg, &|a, d| env.audit.record("system", a, d));
            for e in events {
                match e {
                    Event::SwitchOff => {
                        let _ = env.services.set_announce_name(Some(false));
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
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<(bool, Vec<Svc>, Vec<String>)>>);
    impl Net for Fake {
        fn start(&mut self, label: &str, svcs: Vec<Svc>) -> Result<(), String> {
            let mut g = self.0.lock().unwrap();
            *g = (true, svcs, vec![label.to_string()]);
            Ok(())
        }
        fn set_services(&mut self, svcs: Vec<Svc>) {
            self.0.lock().unwrap().1 = svcs;
        }
        fn set_extra_names(&mut self, names: Vec<String>) {
            let mut g = self.0.lock().unwrap();
            let label = g.2[0].clone();
            g.2 = std::iter::once(label).chain(names).collect();
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().0 = false;
        }
        fn status(&self) -> Option<mdns::net::Status> {
            let g = self.0.lock().unwrap();
            g.0.then(|| mdns::net::Status { running: true, host: format!("{}.local", g.2[0]), probing: false, v4: vec!["192.168.50.66".parse().unwrap()], ..Default::default() })
        }
    }

    fn rig(tag: &str) -> (std::path::PathBuf, Handle, Registry) {
        let dir = std::env::temp_dir().join(format!("iyakki-rdevname-{tag}-{}", std::process::id()));
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

    fn allows_udp(h: &Handle, src: &str) -> bool {
        h.lock().config().rules.iter().any(|r| namerule::is_mdns(r) && r.enabled) && src.starts_with("192.168")
    }

    #[test]
    fn the_rule_and_the_announcer_exist_only_while_the_switch_is_on_and_the_developer_service_follows_developer_mode() {
        let (dir, h, reg) = rig("flow");
        let fake = Fake::default();
        let mut w = Watcher::new(fake.clone());
        let off = Want { on: false, dev_on: true, dev_port: 7979, web_port: None, extra_names: vec![] };
        let (ev, log) = run(&mut w, &off, &dir, &h, &reg);
        assert!(ev.is_empty() && log.is_empty() && !fake.0.lock().unwrap().0 && !h.lock().config().rules.iter().any(namerule::is_mdns));
        // on: the announcer starts with the random label, the rule appears with its reason
        let on = Want { on: true, dev_on: false, dev_port: 7979, web_port: None, extra_names: vec![] };
        let (_, log) = run(&mut w, &on, &dir, &h, &reg);
        assert!(log.iter().any(|l| l.starts_with("devname.firewall.allow: UDP 5353")) && log.iter().any(|l| l.starts_with("devname.announce.start")), "{log:?}");
        assert!(allows_udp(&h, "192.168.50.20"));
        assert!(h.lock().config().rules.iter().filter(|r| namerule::is_mdns(r)).all(|r| r.comment == "device name (mDNS)"));
        let label = fake.0.lock().unwrap().2[0].clone();
        assert!(mdns::is_device_label(&label));
        assert!(fake.0.lock().unwrap().1.is_empty(), "developer mode is off: no developer service");
        assert_eq!(reg.state("devname"), Some(iyakki_agent::startup::StartupState::Ready));
        assert_eq!(devname::read_status(&dir).host, format!("{label}.local"));
        // developer mode on: the service is announced
        run(&mut w, &Want { dev_on: true, ..on.clone() }, &dir, &h, &reg);
        assert_eq!(fake.0.lock().unwrap().1[0].port, 7979);
        // off again: announcer stopped (goodbyes), rule gone, status says so
        run(&mut w, &off, &dir, &h, &reg);
        assert!(!fake.0.lock().unwrap().0 && !h.lock().config().rules.iter().any(namerule::is_mdns));
        assert!(!devname::read_status(&dir).on);
        assert_eq!(reg.state("devname"), Some(iyakki_agent::startup::StartupState::Stopped));
    }

    #[test]
    fn domain_route_names_are_announced_beside_the_device_name_only_while_announcing_is_on() {
        let (dir, h, reg) = rig("extra");
        let fake = Fake::default();
        let mut w = Watcher::new(fake.clone());
        let on = Want { on: true, dev_on: false, dev_port: 7979, web_port: None, extra_names: vec!["store.local".into()] };
        run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(&fake.0.lock().unwrap().2[1..], ["store.local"]);
        run(&mut w, &Want { extra_names: vec![], ..on.clone() }, &dir, &h, &reg);
        assert_eq!(fake.0.lock().unwrap().2.len(), 1, "withdrawn when the listener stops");
        // announcing off: nothing is announced, whatever the routes say
        run(&mut w, &Want { on: false, ..on }, &dir, &h, &reg);
        assert!(!fake.0.lock().unwrap().0);
    }

    #[test]
    fn a_person_removing_the_rule_switches_announcing_off_and_it_is_not_made_again() {
        let (dir, h, reg) = rig("removed");
        let mut w = Watcher::new(Fake::default());
        let on = Want { on: true, dev_on: false, dev_port: 7979, web_port: None, extra_names: vec![] };
        run(&mut w, &on, &dir, &h, &reg);
        // the person removes the rules in the firewall list
        {
            let mut e = h.lock();
            let mut cfg = e.config().clone();
            cfg.rules.retain(|r| !namerule::is_mdns(r));
            e.propose(cfg, "admin").unwrap();
            let _ = e.keep("admin");
        }
        let (ev, log) = run(&mut w, &on, &dir, &h, &reg);
        assert_eq!(ev, vec![Event::SwitchOff]);
        assert!(log.iter().any(|l| l.starts_with("devname.firewall.removed_by_person")));
        assert!(!h.lock().config().rules.iter().any(namerule::is_mdns), "not made again");
    }

    #[test]
    fn a_failure_to_start_is_shown_in_the_startup_list_and_audited_once() {
        struct Broken;
        impl Net for Broken {
            fn start(&mut self, _: &str, _: Vec<Svc>) -> Result<(), String> {
                Err("bind 5353: busy".into())
            }
            fn set_services(&mut self, _: Vec<Svc>) {}
            fn set_extra_names(&mut self, _: Vec<String>) {}
            fn stop(&mut self) {}
            fn status(&self) -> Option<mdns::net::Status> {
                None
            }
        }
        let (dir, h, reg) = rig("fail");
        let mut w = Watcher::new(Broken);
        let on = Want { on: true, dev_on: false, dev_port: 7979, web_port: None, extra_names: vec![] };
        let log = Mutex::new(Vec::new());
        for _ in 0..3 {
            w.tick(&on, &dir, &h, &reg, &|a, d| log.lock().unwrap().push(format!("{a}: {d}")));
        }
        assert_eq!(log.lock().unwrap().iter().filter(|l| l.starts_with("devname.announce.failed")).count(), 1);
        assert!(reg.state("devname").unwrap().failed());
    }
}
