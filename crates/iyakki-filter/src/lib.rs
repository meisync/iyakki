//! The filter matcher shared by the VPN (HYBRID mode), the SSH tunnels and the browser request headers, split out of `iyakki-vpn`
//! (F-BROW-79). `iyakki-vpn` re-exports every module under its old path, so `iyakki_vpn::filter::FilterSet` still works.
//!
//! * [`error`]: [`VpnError`], the error every parser here and in the VPN core returns.
//! * [`text`]: size and character limits and host-name checks.
//! * [`net`]: addresses and networks ([`net::Cidr`]), the route checks.
//! * [`filter`]: domain, URL and address filters, precedence, "why did this match".

pub mod error;
pub mod filter;
pub mod net;
pub mod text;

#[cfg(test)]
mod filter_tests;

pub use error::VpnError;
pub use filter::FilterSet;
pub use net::Cidr;
