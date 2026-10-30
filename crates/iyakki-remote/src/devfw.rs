//! Keeps the firewall's allow rules for developer mode in step with it (`iyakki_fw::devrules` has the rules, `DevMode::fw_want` what
//! is wanted): called every two seconds by the developer-mode watcher. Changes go to the audit log as `devmode.firewall.allow` and
//! `devmode.firewall.removed` (an address, the port and the reason; never a key or a code).

use iyakki_agent::devmode::DevMode;
use iyakki_fw::devrules::{sync, Want};
use iyakki_fw::service::Handle;

/// One look. `last_error` keeps the same failure from being audited every two seconds.
pub fn tick(dm: &DevMode, h: &Handle, audit: &dyn Fn(&str, &str), last_error: &mut String) {
    // a window on somebody else's computer, or a process that never applied the rules, has no firewall of ours to change
    if !h.lock().managed() {
        return;
    }
    let w = dm.fw_want();
    let want = Want { dev_on: w.on, until: w.until, port: w.port, peers: w.peers.iter().filter_map(|p| p.parse().ok()).collect(), pair_until: w.pair_until, network_wide: w.network_wide };
    match sync(h, &want, "developer mode") {
        Ok(changes) => {
            last_error.clear();
            for (a, d) in changes {
                audit(&a, &d);
            }
        }
        Err(e) => {
            if *last_error != e {
                audit("devmode.firewall.failed", &e);
                *last_error = e;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_fw::backend::FakeBackend;
    use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Store};
    use iyakki_fw::devrules::{is_managed, REASON_DEV, REASON_PAIR};
    use iyakki_sec::policy::Role;
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    struct Clk(Arc<AtomicI64>);
    impl Clock for Clk {
        fn now(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
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

    const T0: i64 = 1_900_000_000;

    fn rig(tag: &str) -> (DevMode, Handle, Arc<AtomicI64>) {
        let dir = std::env::temp_dir().join(format!("iyakki-devfw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = Arc::new(AtomicI64::new(T0));
        let c = t.clone();
        let dm = DevMode::open(&dir).with_clock(Arc::new(move || c.load(Ordering::SeqCst)));
        let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(Clk(t.clone())), Box::new(NoDns), Box::new(Mem::default()));
        e.boot(false);
        (dm, Handle::Own(Arc::new(Mutex::new(e))), t)
    }

    fn allows(h: &Handle, src: &str) -> bool {
        h.lock().would_allow_tcp(src.parse().unwrap(), 7979)
    }

    fn run(dm: &DevMode, h: &Handle) -> Vec<String> {
        let log = Mutex::new(Vec::new());
        let mut err = String::new();
        tick(dm, h, &|a, d| log.lock().unwrap().push(format!("{a}: {d}")), &mut err);
        log.into_inner().unwrap()
    }

    #[test]
    fn developer_mode_on_pairing_request_pairing_done_and_off_drive_the_rules_and_the_audit_lines() {
        let (dm, h, _t) = rig("flow");
        let adm = Role::Admin.template();
        // off: nothing
        assert!(run(&dm, &h).is_empty());
        dm.enable(&adm, "root", 1).unwrap();
        assert!(run(&dm, &h).is_empty(), "no computer is known yet: nothing is opened");
        assert!(!allows(&h, "192.168.50.20"));
        // the hint is there; "Allow a computer to pair" opens the local network for the pairing
        assert!(iyakki_fw::devrules::hint(&h.lock(), &[], 7979, true).unwrap().contains("blocks the developer port 7979"));
        dm.start_pairing(&adm, "root").unwrap();
        dm.allow_pairing_through_firewall(&adm, "root").unwrap();
        let log = run(&dm, &h);
        assert!(!log.is_empty() && log.iter().all(|l| l.starts_with("devmode.firewall.allow: TCP 7979 from ") && l.contains(REASON_PAIR)), "{log:?}");
        assert!(!log.iter().any(|l| l.to_lowercase().contains("code")), "no secrets in the audit lines");
        assert!(allows(&h, "192.168.50.20") && allows(&h, "10.9.9.9") && !allows(&h, "8.8.8.8"));
        assert!(h.lock().config().rules.iter().filter(|r| is_managed(r)).all(|r| r.comment == REASON_PAIR));
        // the pairing ends (the first computer paired, here: cancelled) and the computer's own address takes over
        dm.cancel_pairing();
        dm.allow_peer(&adm, "root", "192.168.50.20").unwrap();
        let log = run(&dm, &h);
        assert!(log.iter().any(|l| l.starts_with("devmode.firewall.removed")) && log.iter().any(|l| l.starts_with("devmode.firewall.allow") && l.contains(REASON_DEV)), "{log:?}");
        assert!(allows(&h, "192.168.50.20") && !allows(&h, "10.9.9.9"));
        assert!(run(&dm, &h).is_empty(), "steady");
        assert!(iyakki_fw::devrules::hint(&h.lock(), &[], 7979, true).is_none());
        // off: gone
        dm.disable(&adm, "root").unwrap();
        let log = run(&dm, &h);
        assert!(!log.is_empty() && log.iter().all(|l| l.starts_with("devmode.firewall.removed")), "{log:?}");
        assert!(!allows(&h, "192.168.50.20") && !h.lock().config().rules.iter().any(is_managed));
    }

    #[test]
    fn the_automatic_switch_off_time_removes_the_rules_too_and_a_process_that_applied_no_firewall_is_left_alone() {
        let (dm, h, t) = rig("expiry");
        let adm = Role::Admin.template();
        dm.enable(&adm, "root", 1).unwrap();
        dm.allow_peer(&adm, "root", "192.168.50.20").unwrap();
        run(&dm, &h);
        assert!(allows(&h, "192.168.50.20"));
        t.store(T0 + 3601, Ordering::SeqCst);
        let log = run(&dm, &h);
        assert!(log.iter().all(|l| l.starts_with("devmode.firewall.removed")) && !allows(&h, "192.168.50.20"), "{log:?}");
        // an engine that never booted (a window on somebody else's computer) is not touched
        let e = Engine::new(Box::new(FakeBackend::default()), Box::new(Clk(t.clone())), Box::new(NoDns), Box::new(Mem::default()));
        let idle = Handle::Own(Arc::new(Mutex::new(e)));
        dm.enable(&adm, "root", 0).unwrap();
        assert!(run(&dm, &idle).is_empty());
    }
}
