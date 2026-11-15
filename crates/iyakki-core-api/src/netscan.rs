//! Finding the machines on the local network and what they run (F-ARCH-06, the owner's core rule: the app layer asks the core, the core reads
//! the machine's networks and opens the connections). The data types and the pure address helpers are those of `iyakki-netscan` and are
//! re-exported; the list of this machine's subnets and the scan itself go through [`NetScan`]: the device has `iyakki_core_local::LocalNetScan`,
//! a test has `iyakki_core_fake::FakeNetScan`. Reading the subnets needs `system.read`; a scan opens connections to other machines and
//! therefore needs `net.scan`, a hard ask: a card every time, answered by the person's own press.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

pub use iyakki_netscan::{net, Event, HostInfo, Kind, Options, Service};

pub trait NetScan: Send + Sync {
    /// The subnets this machine is on (interface, address, prefix).
    fn subnets(&self) -> Vec<net::Subnet>;
    /// Start a scan of `targets` and return at once: the answers arrive on `tx` (`Started`, `Progress`, `Host`, `Finished`); setting
    /// `cancel` stops it. Refused (nothing is touched) when the permission is not given.
    fn start(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<Event>) -> Result<(), String>;
}
