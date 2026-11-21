//! Finding other IYAKKI computers on the local network, for the developer-mode client (F-ARCH-07): the name lookup of `name.local`
//! and the scan for the `_iyakki-dev._tcp` announcements are asked through this module, with our own resolver (no system resolver is
//! involved). The agent's client and command line never name the network-scan crate.

use std::net::IpAddr;
use std::time::Duration;

pub use iyakki_netscan::mdns::net::{Ask, Found};
/// The service type a developer-mode device announces.
pub const DEV_SERVICE: &str = iyakki_netscan::mdns::DEV_SERVICE;

/// Is this a `name.local` host name (looked up on the local network rather than by DNS)?
pub fn is_local_name(host: &str) -> bool {
    iyakki_netscan::mdns::is_local_name(host)
}

/// The addresses of `name.local`, waiting up to `wait` for answers.
pub fn resolve(name: &str, wait: Duration) -> Vec<IpAddr> {
    iyakki_netscan::mdns::net::resolve(name, &Ask::new(wait))
}

/// The devices that announce themselves on the local network within `wait`.
pub fn scan(wait: Duration) -> Vec<Found> {
    iyakki_netscan::mdns::net::scan(&Ask::new(wait))
}

/// The same with a given question (unicast destinations instead of the multicast group: tests, or a known responder).
pub fn scan_with(ask: &Ask) -> Vec<Found> {
    iyakki_netscan::mdns::net::scan(ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_dot_local_name_is_looked_up_on_the_network() {
        assert!(is_local_name("iyakki-0a1b2c.local"));
        assert!(is_local_name("Printer.LOCAL"));
        assert!(!is_local_name("example.org"));
        assert!(!is_local_name("192.0.2.5"));
        assert!(!is_local_name(""));
    }

    #[test]
    fn a_responder_that_never_answers_gives_an_empty_scan_and_no_addresses() {
        // the socket stays bound for the whole test, so nothing else can take the port and nothing answers on it
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let ask = Ask { timeout: Duration::from_millis(200), dests: vec![silent.local_addr().unwrap()], allow_loopback: true };
        assert!(scan_with(&ask).is_empty());
        assert_eq!(DEV_SERVICE, "_iyakki-dev._tcp");
    }
}
