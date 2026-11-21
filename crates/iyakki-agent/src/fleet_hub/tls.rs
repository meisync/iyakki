//! The explicit-address transport of the fleet (F-FLEET-08, plan section 3.2: "beyond the LAN: an explicit address over the TLS channel
//! of developer mode"). A member reached across the internet, a VPN or any network the LAN discovery does not cover is named by an address
//! the person gives: `tls://HOST:PORT#FINGERPRINT`. The exchange is the same length-prefixed JSON frame as on the LAN, carried inside TLS 1.3
//! whose certificate must be exactly the one named by the fingerprint (the SHA-256 of the certificate, the pinning developer mode's
//! `iyakki dev pair` uses: the same client settings, `devmode::client::pinned_client_config`). The member messages inside are still
//! signed by the sender's key and sealed with the group key, so TLS here protects the join code and hides the traffic; it does not
//! replace the group's own checks. Without a pin there is no connection: an address without a fingerprint stays on the plain LAN channel.

use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::devmode::auth::norm_fp;

pub const SCHEME: &str = "tls://";

/// Where a member is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// `host:port` on the LAN channel (no TLS: the group's signatures and sealing protect the messages).
    Plain(String),
    /// `tls://host:port#fingerprint`: over TLS, the certificate pinned.
    Tls { addr: String, fingerprint: String },
    /// `relay://MAILBOX`: through the relay of F-CORE-17 (F-FLEET-10).
    Relay(String),
}

impl Target {
    pub fn parse(s: &str) -> Result<Target, String> {
        let s = s.trim();
        if let Some(m) = s.strip_prefix(super::relay::SCHEME) {
            return if super::relay::valid_mailbox(m) { Ok(Target::Relay(m.to_string())) } else { Err("a relay:// address is the member's mailbox, 32 lowercase hex digits".into()) };
        }
        let Some(rest) = s.strip_prefix(SCHEME) else {
            return Ok(Target::Plain(s.to_string()));
        };
        let (addr, fp) = rest.split_once('#').ok_or("a tls:// address needs the certificate fingerprint after #, like tls://host:7671#AB:CD:..")?;
        let fp = norm_fp(fp);
        if fp.len() != 64 {
            return Err("the fingerprint must be 64 hex digits (the SHA-256 of the certificate)".into());
        }
        let ok_port = addr.rsplit_once(':').is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok_and(|p| p != 0));
        if !ok_port || addr.len() > 200 || addr.chars().any(|c| c.is_control() || c.is_whitespace() || c == '#') {
            return Err("that address is not valid (use tls://host:port#fingerprint)".into());
        }
        Ok(Target::Tls { addr: addr.to_string(), fingerprint: fp })
    }

    /// The text form saved and shown (the fingerprint in the colon form people compare).
    pub fn text(&self) -> String {
        match self {
            Target::Plain(a) => a.clone(),
            Target::Tls { addr, fingerprint } => format!("{SCHEME}{addr}#{}", colon(fingerprint)),
            Target::Relay(m) => format!("{}{m}", super::relay::SCHEME),
        }
    }
}

/// `ABCD..` as `AB:CD:..`.
pub fn colon(fp: &str) -> String {
    norm_fp(fp).as_bytes().chunks(2).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(":")
}

/// The address text a member advertises for itself: this host as the other side saw it, the port and the certificate to pin.
pub fn advertised(ip: std::net::IpAddr, port: u16, fingerprint: &str) -> String {
    let host = match ip {
        std::net::IpAddr::V6(v) => format!("[{v}]"),
        std::net::IpAddr::V4(v) => v.to_string(),
    };
    format!("{SCHEME}{host}:{port}#{}", colon(fingerprint))
}

/// A TCP connection to `addr` (host:port): every address the name resolves to is tried in turn, the way `TcpStream::connect` does, not only
/// the first. A name that resolves to an IPv6 and an IPv4 address (`localhost` on many systems lists `::1` first) must still reach a
/// member that listens on one of them.
pub fn connect_tcp(addr: &str) -> Result<std::net::TcpStream, String> {
    let all: Vec<std::net::SocketAddr> = addr.to_socket_addrs().map_err(|_| "that address is not valid (use host:port)".to_string())?.collect();
    connect_first_of(&all).map_err(|e| format!("could not reach {addr}: {e}"))
}

/// The first of `addrs` that accepts a connection (3 seconds each); the last error when none does.
pub fn connect_first_of(addrs: &[std::net::SocketAddr]) -> Result<std::net::TcpStream, String> {
    let mut last = "that address is not valid".to_string();
    for sa in addrs {
        match std::net::TcpStream::connect_timeout(sa, Duration::from_secs(3)) {
            Ok(s) => return Ok(s),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// A TLS connection to `addr`, the certificate pinned to `fingerprint`. The handshake runs on the first read or write, so a wrong
/// certificate is refused before one byte of the message is sent (the client sends nothing until the verifier has accepted).
pub fn connect(addr: &str, fingerprint: &str) -> Result<rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>, String> {
    let sock = connect_tcp(addr).map_err(|e| if e.starts_with("that address") { "that address is not valid (use tls://host:port#fingerprint)".to_string() } else { e })?;
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(5)));
    let cfg = crate::devmode::client::pinned_client_config(Some(fingerprint), Arc::new(Mutex::new(None)))?;
    let name = rustls::pki_types::ServerName::try_from("iyakki-device").map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(cfg, name).map_err(|e| e.to_string())?;
    Ok(rustls::StreamOwned::new(conn, sock))
}

/// What a refused handshake says to the person, in plain words.
pub fn explain(e: &str) -> String {
    if e.contains("certificate changed") || e.contains("General") || e.contains("invalid peer") || e.contains("received fatal alert") {
        "the certificate that computer showed is not the one named in the address (check the fingerprint on its Fleet page)".into()
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod connect_tests {
    use super::*;

    #[test]
    fn a_name_with_several_addresses_reaches_the_one_that_listens_not_only_the_first() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let good = l.local_addr().unwrap();
        // port 0 can never be connected to: it stands for the first address of a name that does not listen (an IPv6 `::1` before the IPv4 one)
        let dead: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let s = connect_first_of(&[dead, good]).unwrap();
        assert_eq!(s.peer_addr().unwrap(), good);
        assert!(connect_first_of(&[dead]).is_err() && connect_first_of(&[]).unwrap_err().contains("not valid"));
        assert!(connect_tcp("not an address").unwrap_err().contains("not valid"));
    }
}
