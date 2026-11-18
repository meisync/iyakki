//! Internet sharing ("tethering"): this computer gives other computers the internet it has, over a cable. Wired Ethernet, a USB network
//! link (gadget, RNDIS/ECM adapter) or a Thunderbolt / USB-C link; a Wi-Fi access point later (plan: docs/development/HOTSPOT-PLAN.md).
//!
//! Layout (the pure parts first, the machine last):
//!
//! * [`plan`]: which uplink, which downstream interface, which private /24; every refusal as one plain line ([`ShareError`]).
//! * [`rules`]: the firewall table `inet iyakki_share` as typed values for the firewall's own netlink writer (no `nft` program, no shell).
//! * [`manager`]: the state machine (start, idle switch-off, uplink loss, clean-up after a crash) over the [`manager::Os`] and
//!   [`manager::Io`] traits. DHCP and DNS are `iyakki_hotspot`'s byte-level servers.
//! * [`io`]: the sockets (a [`io::SocketMaker`] trait; the real one binds with `SO_BINDTODEVICE`), generic so tests use fakes and loopback.
//! * [`real`]: the machine itself (`/sys/class/net`, `/proc`, interface ioctls, the firewall socket). [`cli`]: `iyakki share`.
//! * [`control`]: the request and status files that the shell and the command line use to talk to the service (and a backend trait for
//!   the Settings page).
//!
//! Safe by default: sharing is off after every boot; devices on the cable can only ask for an address and for names (the device's own
//! services stay closed to them); they cannot reach each other or this computer's local network unless an administrator says so; and it
//! switches itself off after N hours (default 2) with nobody using it.

pub mod cli;
pub mod control;
pub mod io;
pub mod manager;
pub mod notice;
pub mod real;
pub mod rules;

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;

pub use iyakki_shared::sharing::*;
