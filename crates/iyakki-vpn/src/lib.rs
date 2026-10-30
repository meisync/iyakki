//! The VPN core of IYAKKI. Design, threat list and step plan: `docs/development/VPN-CONNECTOR-PLAN.md`.
//!
//! Everything here is pure or runs against a fake; nothing opens a socket, starts a program or needs root. The helper
//! process, the store, the kernel module and the UI come in later steps (see the Status section of the plan).
//!
//! * [`text`]: the size and character limits every imported file passes first.
//! * [`net`]: addresses and networks ([`net::Cidr`]), the route checks of the threat list.
//! * [`secret`]: keys and passwords that are zeroised on drop and never printed.
//! * [`profile`]: the typed profile (kind WireGuard or OpenVPN) without any secret.
//! * [`wgconf`]: `wg-quick` `.conf` parser and sanitiser. [`ovpn`]: `.ovpn` allow-list sanitiser.
//! * [`wgnl`]: WireGuard generic-netlink messages (`SET_DEVICE`, `GET_DEVICE`), a reply decoder, a transport trait and a fake.
//! * [`inspect`]: `iyakki vpn inspect FILE`, an offline report on a profile file that never prints a value.
//! * [`filter`]: HYBRID mode filters (domain, URL, address), precedence, "why did this match".
//! * [`dnswire`], [`dnsroute`]: the DNS forwarder as a pure state machine that turns answers into routes.
//! * [`plan`]: routes (FULL and HYBRID), DNS text, the DNS capture rule, the kill switch and the fail-closed guard as pure data.

pub mod b64;
pub mod dnsroute;
pub mod dnswire;
pub mod error;
pub mod filter;
pub mod inspect;
pub mod net;
pub mod ovpn;
pub mod plan;
pub mod profile;
pub mod secret;
pub mod text;
pub mod wgconf;
pub mod wgnl;

#[cfg(test)]
mod dns_tests;
#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod inspect_tests;
#[cfg(test)]
mod ovpn_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod profile_tests;
#[cfg(test)]
mod wgconf_tests;
#[cfg(test)]
mod wgnl_tests;

pub use error::VpnError;
pub use net::Cidr;
pub use filter::FilterSet;
pub use profile::{Kind, Mode, Profile, Settings};
pub use secret::Secret;
