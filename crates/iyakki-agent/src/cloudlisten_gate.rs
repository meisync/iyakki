//! The firewall behind [`PortGate`]: a port that gains its first route is opened (a TCP allow rule, `iyakki_fw::gaterule`), and closed
//! again when its last route goes or the listener stops. The rules are the firewall's own typed rules (no text reaches the kernel), kept
//! with the reason "domain listener (closes with its last route)" and applied without the 60-second question because they only open TCP
//! ports the person configured routes for.
//!
//! By default a port is open to the local network only; [`Reach::Anywhere`] has to be asked for (a listener with public routes).
//! The decision "which ports" stays the listener's; this file only turns it into rules. The apply step is a function, so tests record
//! what would be applied instead of touching the machine's firewall; [`FirewallGate::dry_run`] renders the resulting ruleset as text.

use std::collections::BTreeSet;
use std::sync::Mutex;

use iyakki_fw::gaterule::{self, Reach};
use iyakki_fw::ruleset::Ctx;
use iyakki_fw::service::Handle;
use iyakki_fw::Config;

use super::PortGate;

pub use iyakki_fw::gaterule::Reach as GateReach;

type Apply = Box<dyn Fn(&BTreeSet<u16>, Reach) -> Result<(), String> + Send + Sync>;

pub struct FirewallGate {
    ports: Mutex<BTreeSet<u16>>,
    reach: Reach,
    apply: Apply,
}

impl FirewallGate {
    /// Drive the machine's real firewall (the process-wide engine). Not used by tests.
    pub fn real(reach: Reach) -> FirewallGate {
        FirewallGate::with_apply(reach, Box::new(|ports, reach| gaterule::sync(&Handle::Global, ports, reach, "cloud listen").map(|_| ())))
    }

    /// The real firewall, the opening ending at `expires` (Unix seconds; 0: no end time). Used by the Domains page.
    pub fn real_until(reach: Reach, expires: i64) -> FirewallGate {
        FirewallGate::with_apply(reach, Box::new(move |ports, reach| gaterule::sync_until(&Handle::Global, ports, reach, "domains", expires).map(|_| ())))
    }

    /// Drive a given engine (the firewall's own tests use a fake backend).
    pub fn on(handle: Handle, reach: Reach) -> FirewallGate {
        FirewallGate::with_apply(reach, Box::new(move |ports, reach| gaterule::sync(&handle, ports, reach, "cloud listen").map(|_| ())))
    }

    pub fn with_apply(reach: Reach, apply: Apply) -> FirewallGate {
        FirewallGate { ports: Mutex::new(BTreeSet::new()), reach, apply }
    }

    /// The ports open right now.
    pub fn open_ports(&self) -> Vec<u16> {
        self.ports.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect()
    }

    /// What the firewall would look like with `ports` open on top of `base`, as text. Nothing is applied.
    pub fn dry_run(base: &Config, ports: &[u16], reach: Reach, ctx: &Ctx) -> Result<String, String> {
        gaterule::render(base, &ports.iter().copied().collect(), reach, ctx)
    }
}

impl PortGate for FirewallGate {
    fn open(&self, port: u16) -> Result<(), String> {
        if port == 0 {
            return Err("port 0 is not a port".into());
        }
        let mut set = self.ports.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = set.clone();
        next.insert(port);
        (self.apply)(&next, self.reach)?;
        *set = next;
        Ok(())
    }

    fn close(&self, port: u16) {
        let mut set = self.ports.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = set.clone();
        next.remove(&port);
        // a failure to close cannot be reported to the caller; the port stays in the set so the next change tries again
        if (self.apply)(&next, self.reach).is_ok() {
            *set = next;
        }
    }
}
