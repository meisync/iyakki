//! What the NET SCAN tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (the `iyakki-tile-scan` crate) names no `Services`, no session and no audit log: it asks a [`ScanPort`] for the machine's subnets and the scanner
//! (the core's permission-checked `NetScan`), the account's host policy, the new-device watch, the audit line and, for administrators, adding a
//! found model server to Settings -> Models. The real port ([`CxPort`]) is the only place that reads the session, the vault and the settings.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use iyakki_core_api::netscan::{net, Event as ScanEvent, Options, Service};
use iyakki_tile_api::scan as t;

use super::scan_models::{add_model_server, CxModels};
use super::AppCx;

pub trait ScanPort {
    /// The subnets this machine is on.
    fn subnets(&self) -> Vec<net::Subnet>;
    /// Start a scan of ; refused (nothing is touched) when the permission is not given.
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<ScanEvent>) -> Result<(), String>;
    /// The account's host policy lets it touch .
    fn allows_host(&self, ip: &str) -> bool;
    /// The account may change settings (administrators).
    fn admin(&self) -> bool;
    /// The new-device watch.
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Add a found model server to Settings -> Models (administrators); what to tell the person.
    fn add_model_server(&self, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String>;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl ScanPort for CxPort<'_> {
    fn subnets(&self) -> Vec<net::Subnet> {
        self.0.os.netscan().subnets()
    }
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: Options, cancel: Arc<AtomicBool>, tx: Sender<ScanEvent>) -> Result<(), String> {
        self.0.os.netscan().start(targets, opts, cancel, tx)
    }
    fn allows_host(&self, ip: &str) -> bool {
        self.0.session.policy.allows_host(ip)
    }
    fn admin(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn devwatch(&self) -> Arc<dyn iyakki_agent_api::DevWatchApi> {
        self.0.devwatch()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn add_model_server(&self, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
        let svc = svc_to_tile(svc);
        add_model_server(&CxModels(self.0), ip, &svc, id, address, model, key)
    }
}


// ------------------------------------------------------------------------------------------------ the tile crate's port (D-S1-E4-01, pattern b)

fn kind_to_tile(k: iyakki_core_api::netscan::Kind) -> t::Kind {
    use iyakki_core_api::netscan::Kind as K;
    match k {
        K::Ollama => t::Kind::Ollama,
        K::LlmServer => t::Kind::LlmServer,
        K::Camera => t::Kind::Camera,
        K::Media => t::Kind::Media,
        K::Printer => t::Kind::Printer,
        K::SmartHome => t::Kind::SmartHome,
        K::Data => t::Kind::Data,
        K::Device => t::Kind::Device,
        K::Access => t::Kind::Access,
        K::Platform => t::Kind::Platform,
        K::Web => t::Kind::Web,
        K::Other => t::Kind::Other,
    }
}

fn kind_from_tile(k: t::Kind) -> iyakki_core_api::netscan::Kind {
    use iyakki_core_api::netscan::Kind as K;
    match k {
        t::Kind::Ollama => K::Ollama,
        t::Kind::LlmServer => K::LlmServer,
        t::Kind::Camera => K::Camera,
        t::Kind::Media => K::Media,
        t::Kind::Printer => K::Printer,
        t::Kind::SmartHome => K::SmartHome,
        t::Kind::Data => K::Data,
        t::Kind::Device => K::Device,
        t::Kind::Access => K::Access,
        t::Kind::Platform => K::Platform,
        t::Kind::Web => K::Web,
        t::Kind::Other => K::Other,
    }
}

fn confidence_to_tile(c: iyakki_core_api::device_types::netscan::Confidence) -> t::Confidence {
    use iyakki_core_api::device_types::netscan::Confidence as C;
    match c {
        C::Low => t::Confidence::Low,
        C::Medium => t::Confidence::Medium,
        C::High => t::Confidence::High,
    }
}

fn confidence_from_tile(c: t::Confidence) -> iyakki_core_api::device_types::netscan::Confidence {
    use iyakki_core_api::device_types::netscan::Confidence as C;
    match c {
        t::Confidence::Low => C::Low,
        t::Confidence::Medium => C::Medium,
        t::Confidence::High => C::High,
    }
}

pub(super) fn svc_to_tile(s: &Service) -> t::Service {
    t::Service { port: s.port, proto: s.proto, kind: kind_to_tile(s.kind), name: s.name.clone(), detail: s.detail.clone(), confidence: confidence_to_tile(s.confidence), risk: s.risk.clone(), models: s.models.clone(), url: s.url.clone() }
}

pub(super) fn svc_from_tile(s: &t::Service) -> Service {
    Service { port: s.port, proto: s.proto, kind: kind_from_tile(s.kind), name: s.name.clone(), detail: s.detail.clone(), confidence: confidence_from_tile(s.confidence), risk: s.risk.clone(), models: s.models.clone(), url: s.url.clone() }
}

pub(super) fn host_to_tile(h: &iyakki_core_api::netscan::HostInfo) -> t::HostInfo {
    t::HostInfo { ip: h.ip, mac: h.mac.clone(), name: h.name.clone(), services: h.services.iter().map(svc_to_tile).collect() }
}

pub(super) fn host_from_tile(h: &t::HostInfo) -> iyakki_core_api::netscan::HostInfo {
    iyakki_core_api::netscan::HostInfo { ip: h.ip, mac: h.mac.clone(), name: h.name.clone(), services: h.services.iter().map(svc_from_tile).collect() }
}

fn subnet_to_tile(s: net::Subnet) -> t::Subnet {
    t::Subnet { iface: s.iface, addr: s.addr, prefix: s.prefix }
}

fn subnet_from_tile(s: &t::Subnet) -> net::Subnet {
    net::Subnet { iface: s.iface.clone(), addr: s.addr, prefix: s.prefix }
}

pub(super) fn event_to_tile(e: ScanEvent) -> t::Event {
    match e {
        ScanEvent::Started { hosts, ports } => t::Event::Started { hosts, ports },
        ScanEvent::Progress { done, total } => t::Event::Progress { done, total },
        ScanEvent::Host(h) => t::Event::Host(host_to_tile(&h)),
        ScanEvent::Finished { hosts, services, cancelled } => t::Event::Finished { hosts, services, cancelled },
    }
}

/// The scanner's channel as the tile's feed: every answer turned into the tile's plain type as it is taken.
struct Feed(std::sync::mpsc::Receiver<ScanEvent>);

impl t::ScanFeed for Feed {
    fn next(&mut self) -> Option<t::Event> {
        self.0.try_recv().ok().map(event_to_tile)
    }
}

/// The new-device watch as the tile's watch.
struct Watch(Arc<dyn iyakki_agent_api::DevWatchApi>);

fn device_to_tile(d: iyakki_shared::devwatch::Device) -> t::Device {
    t::Device { mac: d.mac, ip: d.ip, iface: d.iface, first_seen: d.first_seen, last_seen: d.last_seen, trusted: d.trusted, name: d.name }
}

impl t::DevWatch for Watch {
    fn enabled(&self) -> bool {
        self.0.enabled()
    }
    fn set_enabled(&self, on: bool) {
        self.0.set_enabled(on)
    }
    fn observe_scan(&self, seen: &[t::Neighbor], now: i64) {
        let seen: Vec<iyakki_shared::devwatch::Neighbor> = seen.iter().map(|n| iyakki_shared::devwatch::Neighbor { ip: n.ip.clone(), mac: n.mac.clone(), iface: n.iface.clone() }).collect();
        self.0.observe_scan(&seen, now);
    }
    fn list(&self) -> Vec<t::Device> {
        self.0.list().into_iter().map(device_to_tile).collect()
    }
    fn untrusted_count(&self) -> usize {
        self.0.untrusted_count()
    }
    fn trust(&self, mac: &str) -> bool {
        self.0.trust(mac)
    }
    fn trust_all(&self) -> usize {
        self.0.trust_all()
    }
    fn rename(&self, mac: &str, name: &str) -> bool {
        self.0.rename(mac, name)
    }
    fn forget(&self, mac: &str) -> bool {
        self.0.forget(mac)
    }
}

/// The shell's port as the tile crate's port.
pub struct Bridge<'a>(pub &'a dyn ScanPort);

impl t::ScanPort for Bridge<'_> {
    fn subnets(&self) -> Vec<t::Subnet> {
        self.0.subnets().into_iter().map(subnet_to_tile).collect()
    }
    fn hosts_of(&self, s: &t::Subnet) -> Vec<Ipv4Addr> {
        net::hosts_of(&subnet_from_tile(s))
    }
    fn parse_targets(&self, text: &str) -> Result<Vec<Ipv4Addr>, String> {
        net::parse_targets(text)
    }
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: t::ScanOpts, cancel: Arc<AtomicBool>) -> Result<Box<dyn t::ScanFeed>, String> {
        let mut o = Options::default();
        if let Some(p) = opts.ports {
            o.ports = p;
            o.announce_wait = None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.0.start_scan(targets, o, cancel, tx)?;
        Ok(Box::new(Feed(rx)))
    }
    fn allows_host(&self, ip: &str) -> bool {
        self.0.allows_host(ip)
    }
    fn admin(&self) -> bool {
        self.0.admin()
    }
    fn devwatch(&self) -> Arc<dyn t::DevWatch> {
        Arc::new(Watch(self.0.devwatch()))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit(action, subject)
    }
    fn add_model_server(&self, ip: Ipv4Addr, svc: &t::Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String> {
        self.0.add_model_server(ip, &svc_from_tile(svc), id, address, model, key)
    }
    fn now(&self) -> i64 {
        iyakki_clock::now_unix()
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;
    use iyakki_core_api::device_types::netscan::Confidence;
    use iyakki_core_api::netscan::Kind;

    const KINDS: [Kind; 12] = [Kind::Ollama, Kind::LlmServer, Kind::Camera, Kind::Media, Kind::Printer, Kind::SmartHome, Kind::Data, Kind::Device, Kind::Access, Kind::Platform, Kind::Web, Kind::Other];

    #[test]
    fn kinds_have_the_same_labels_model_flag_and_order_in_both_crates_and_round_trip() {
        for (i, k) in KINDS.iter().enumerate() {
            let m = kind_to_tile(*k);
            assert_eq!(k.label(), m.label());
            assert_eq!(k.is_model_server(), m.is_model_server());
            assert_eq!(kind_from_tile(m), *k);
            if let Some(next) = KINDS.get(i + 1) {
                assert!(m < kind_to_tile(*next), "the order is the same: {k:?} before {next:?}");
            }
        }
    }

    #[test]
    fn confidence_keeps_its_order_and_round_trips() {
        for (a, b) in [(Confidence::Low, Confidence::Medium), (Confidence::Medium, Confidence::High)] {
            assert!(confidence_to_tile(a) < confidence_to_tile(b));
            assert_eq!(confidence_from_tile(confidence_to_tile(a)), a);
        }
        assert_eq!(confidence_from_tile(confidence_to_tile(Confidence::High)), Confidence::High);
    }

    #[test]
    fn a_host_with_services_round_trips_and_the_helpers_agree() {
        let mut h = iyakki_core_api::netscan::HostInfo::new(Ipv4Addr::new(10, 0, 0, 7));
        h.mac = Some("aa:bb:cc:dd:ee:ff".into());
        h.name = Some("tv".into());
        let mut cam = Service::new(554, Kind::Camera, "Camera");
        cam.risk = Some("no password".into());
        cam.url = Some("rtsp://10.0.0.7:554/".into());
        cam.detail = "d".into();
        cam.confidence = Confidence::High;
        let mut ol = Service::new(11434, Kind::Ollama, "Ollama");
        ol.models = vec!["m".into()];
        h.add(cam);
        h.add(ol);
        let m = host_to_tile(&h);
        assert_eq!(host_from_tile(&m), h);
        assert_eq!((m.main_kind(), m.risky(), m.has(t::Kind::Camera)), (kind_to_tile(h.main_kind()), h.risky(), h.has(Kind::Camera)));
    }

    #[test]
    fn events_are_converted_one_for_one() {
        assert_eq!(event_to_tile(ScanEvent::Started { hosts: 3, ports: 4 }), t::Event::Started { hosts: 3, ports: 4 });
        assert_eq!(event_to_tile(ScanEvent::Progress { done: 1, total: 12 }), t::Event::Progress { done: 1, total: 12 });
        assert_eq!(event_to_tile(ScanEvent::Finished { hosts: 2, services: 5, cancelled: true }), t::Event::Finished { hosts: 2, services: 5, cancelled: true });
        let h = iyakki_core_api::netscan::HostInfo::new(Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(event_to_tile(ScanEvent::Host(h.clone())), t::Event::Host(host_to_tile(&h)));
    }

    #[test]
    fn a_subnet_has_the_same_cidr_in_both_crates() {
        for (addr, prefix) in [(Ipv4Addr::new(192, 168, 1, 77), 24), (Ipv4Addr::new(10, 1, 2, 3), 8), (Ipv4Addr::new(172, 16, 5, 9), 22), (Ipv4Addr::new(1, 2, 3, 4), 0)] {
            let s = net::Subnet { iface: "eth0".into(), addr, prefix };
            assert_eq!(s.cidr(), subnet_to_tile(s.clone()).cidr());
            assert_eq!(subnet_from_tile(&subnet_to_tile(s.clone())), s);
        }
    }
}
