//! Name and address checks the routes share (moved out of the agent's cloudtenant, cloudstorage and cloudlisten_tunnel for F-SPLIT-03).

use std::net::IpAddr;

pub fn dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_lowercase()
        && *b.last().unwrap_or(&b'-') != b'-'
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn dns_subdomain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(|p| !p.is_empty() && p.len() <= 63 && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') && !p.starts_with('-') && !p.ends_with('-'))
}

pub fn host_name(s: &str) -> bool {
    s.contains('.') && dns_subdomain(s)
}


/// A name that is one plain path component: letters, digits, `-` and `_`, not empty, not starting with a dot.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !s.starts_with('.')
}

/// Private, not loopback, not link-local, not unspecified.
pub fn is_private(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(v) => {
            let o = v.octets();
            !v.is_loopback() && !v.is_link_local() && !v.is_unspecified() && (o[0] == 10 || (o[0] == 172 && (16..32).contains(&o[1])) || (o[0] == 192 && o[1] == 168) || (o[0] == 100 && (64..128).contains(&o[1])))
        }
        IpAddr::V6(v) => !v.is_loopback() && !v.is_unspecified() && (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Check a `peer` target's address when the route table is read: an address, private, and a port.
pub fn check_peer(host: &str, addr: &str, port: u16) -> Result<(), String> {
    let ip: IpAddr = addr.parse().map_err(|_| format!("route {host}: \"{addr}\" is not an IP address"))?;
    if !is_private(ip) {
        return Err(format!("route {host}: {addr} is not a private address (a peer is reached over a VPN)"));
    }
    if port == 0 {
        return Err(format!("route {host}: port 0 is not a port"));
    }
    Ok(())
}
