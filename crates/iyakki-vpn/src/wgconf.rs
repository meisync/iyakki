//! `wg-quick` `.conf` import. An allow-list: every key must be known; the ones that run commands or change routing tables
//! are refused with the reason. The private key and preshared keys are moved into [`ProfileSecrets`]; error messages
//! never contain a value.

use std::net::IpAddr;

use crate::error::{invalid, rejected, VpnError};
use crate::net::{check_dns, check_endpoint, check_route, Cidr, Context, EndpointVerdict, RouteVerdict};
use crate::profile::{Body, Endpoint, Imported, PublicKey, WgConfig, WgPeer};
use crate::secret::{ProfileSecrets, Secret};
use crate::text;

pub const MAX_PEERS: usize = 8;

#[derive(PartialEq)]
enum Section {
    None,
    Interface,
    Peer,
}

/// `host:port` or `[v6]:port`. The value is not echoed in errors.
pub fn parse_endpoint(v: &str) -> Option<Endpoint> {
    let v = v.trim();
    let (host, port) = if let Some(rest) = v.strip_prefix('[') {
        let (h, p) = rest.split_once("]:")?;
        if h.parse::<std::net::Ipv6Addr>().is_err() {
            return None;
        }
        (h, p)
    } else {
        let (h, p) = v.rsplit_once(':')?;
        if h.contains(':') {
            return None; // an unbracketed IPv6 address is ambiguous
        }
        (h, p)
    };
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|&p| p != 0)?;
    if host.parse::<IpAddr>().is_err() {
        if !text::valid_hostname(host) {
            return None;
        }
        // a name whose last label is only digits is a mistyped address
        if host.trim_end_matches('.').rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
    }
    Some(Endpoint { host: host.to_string(), port })
}

/// The setting names the importer knows (lower case); `inspect` prints only these.
pub const KNOWN_KEYS: &[&str] = &["privatekey", "address", "dns", "mtu", "listenport", "fwmark", "preup", "postup", "predown", "postdown", "saveconfig", "table", "publickey", "presharedkey", "endpoint", "allowedips", "persistentkeepalive"];

/// A directive name that is safe to put in a message.
pub(crate) fn shown(k: &str) -> &str {
    if k.len() <= 32 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        k
    } else {
        "(setting)"
    }
}

fn num<T: std::str::FromStr>(v: &str) -> Option<T> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}

fn list(v: &str) -> impl Iterator<Item = &str> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty())
}

struct Peer {
    public: Option<PublicKey>,
    preshared: Option<Secret>,
    endpoint: Option<Endpoint>,
    allowed: Vec<Cidr>,
    keepalive: Option<u16>,
    line: usize,
}

/// Parse and sanitise. `ctx` carries the cluster CIDRs (and the LAN if known) for the route checks; the checks are repeated
/// with the real network at connect time by [`crate::plan`].
pub fn parse(name: &str, input: &[u8], ctx: &Context) -> Result<Imported, VpnError> {
    let name = text::check_name(name)?;
    let src = text::clean(input)?;
    let mut warnings = Vec::new();

    let mut section = Section::None;
    let mut seen_interface = false;
    let mut private_key: Option<Secret> = None;
    let mut addresses: Vec<Cidr> = Vec::new();
    let mut dns_raw: Vec<String> = Vec::new();
    let mut mtu = None;
    let mut listen_port = None;
    let mut peers: Vec<Peer> = Vec::new();

    for (i, raw) in src.split('\n').enumerate() {
        let ln = i + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            match line.to_ascii_lowercase().as_str() {
                "[interface]" => {
                    if seen_interface {
                        return Err(invalid(ln, "[Interface]", "there can be only one Interface section"));
                    }
                    seen_interface = true;
                    section = Section::Interface;
                }
                "[peer]" => {
                    if peers.len() >= MAX_PEERS {
                        return Err(invalid(ln, "[Peer]", "too many peers"));
                    }
                    peers.push(Peer { public: None, preshared: None, endpoint: None, allowed: Vec::new(), keepalive: None, line: ln });
                    section = Section::Peer;
                }
                _ => return Err(invalid(ln, "section", "only [Interface] and [Peer] exist")),
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(invalid(ln, "line", "expected Key = Value"));
        };
        let (k, v) = (k.trim(), v.trim());
        let key = k.to_ascii_lowercase();
        // a pasted bare key ends in '=', so the "name" may be a secret: only plain names are ever shown
        let k = shown(k);
        match section {
            Section::None => return Err(invalid(ln, k, "a setting before any section")),
            Section::Interface => match key.as_str() {
                "privatekey" => {
                    if private_key.is_some() {
                        return Err(invalid(ln, k, "given twice"));
                    }
                    private_key = Some(Secret::from_key_b64(v).ok_or_else(|| invalid(ln, k, "not a 32-byte base64 key"))?);
                }
                "address" => {
                    for a in list(v) {
                        addresses.push(Cidr::parse(a).ok_or_else(|| invalid(ln, k, "not an address with an optional prefix"))?);
                    }
                }
                "dns" => dns_raw.extend(list(v).map(str::to_string)),
                "mtu" => {
                    let m: u16 = num(v).filter(|m| (576..=9000).contains(m)).ok_or_else(|| invalid(ln, k, "must be a number from 576 to 9000"))?;
                    mtu = Some(m);
                }
                "listenport" => listen_port = Some(num::<u16>(v).ok_or_else(|| invalid(ln, k, "not a port number"))?),
                "fwmark" => {
                    if !(v.eq_ignore_ascii_case("off") || v == "0") {
                        return Err(rejected(ln, k, "firewall marks are for policy routing, which these images do not have"));
                    }
                }
                "preup" | "postup" | "predown" | "postdown" => {
                    return Err(rejected(ln, k, "it would run a command as root"));
                }
                "saveconfig" => return Err(rejected(ln, k, "it would rewrite the profile from the running state")),
                "table" => return Err(rejected(ln, k, "IYAKKI sets the routes itself and does not use routing tables")),
                _ => return Err(rejected(ln, k, "not on the list of supported settings")),
            },
            Section::Peer => {
                let p = peers.last_mut().expect("a peer section opened this");
                match key.as_str() {
                    "publickey" => {
                        if p.public.is_some() {
                            return Err(invalid(ln, k, "given twice"));
                        }
                        p.public = Some(PublicKey::parse(v).ok_or_else(|| invalid(ln, k, "not a 32-byte base64 key"))?);
                    }
                    "presharedkey" => {
                        if p.preshared.is_some() {
                            return Err(invalid(ln, k, "given twice"));
                        }
                        p.preshared = Some(Secret::from_key_b64(v).ok_or_else(|| invalid(ln, k, "not a 32-byte base64 key"))?);
                    }
                    "endpoint" => {
                        if p.endpoint.is_some() {
                            return Err(invalid(ln, k, "given twice"));
                        }
                        p.endpoint = Some(parse_endpoint(v).ok_or_else(|| invalid(ln, k, "expected host:port"))?);
                    }
                    "allowedips" => {
                        for a in list(v) {
                            let c = Cidr::parse(a).ok_or_else(|| invalid(ln, k, "not an address with an optional prefix"))?;
                            p.allowed.push(c.network());
                        }
                    }
                    "persistentkeepalive" => {
                        p.keepalive = if v.eq_ignore_ascii_case("off") { None } else { Some(num::<u16>(v).ok_or_else(|| invalid(ln, k, "seconds or off"))?) }.filter(|&s| s != 0);
                    }
                    "preup" | "postup" | "predown" | "postdown" | "table" | "saveconfig" => {
                        return Err(rejected(ln, k, "not allowed in a peer section"));
                    }
                    _ => return Err(rejected(ln, k, "not on the list of supported settings")),
                }
            }
        }
    }

    if !seen_interface {
        return Err(VpnError::Missing("the profile has no [Interface] section".into()));
    }
    let private_key = private_key.ok_or_else(|| VpnError::Missing("the [Interface] section has no PrivateKey".into()))?;
    if addresses.is_empty() {
        return Err(VpnError::Missing("the [Interface] section has no Address".into()));
    }
    if peers.is_empty() {
        return Err(VpnError::Missing("the profile has no [Peer] section".into()));
    }

    // DNS: names are search domains in wg-quick; we use addresses only.
    let mut dns_addrs = Vec::new();
    for d in dns_raw {
        if crate::net::parse_ip(&d).is_some() {
            dns_addrs.push(d);
        } else {
            warnings.push("a DNS search domain in the profile was ignored; only DNS server addresses are used".to_string());
        }
    }
    let (dns, dns_warn) = check_dns(&dns_addrs, ctx)?;
    warnings.extend(dns_warn);

    // The tunnel's own networks, so AllowedIPs inside them are not flagged as foreign private ranges.
    let mut rctx = ctx.clone();
    rctx.tunnel.extend(addresses.iter().map(|a| a.network()));

    let mut out_peers = Vec::new();
    let mut secrets = ProfileSecrets { private_key: Some(private_key), ..Default::default() };
    for (idx, p) in peers.into_iter().enumerate() {
        let public_key = p.public.ok_or_else(|| VpnError::Missing(format!("the [Peer] section at line {} has no PublicKey", p.line)))?;
        if p.allowed.is_empty() {
            return Err(VpnError::Missing(format!("the [Peer] section at line {} has no AllowedIPs", p.line)));
        }
        for r in &p.allowed {
            match check_route(r, &rctx) {
                RouteVerdict::Ok => {}
                RouteVerdict::Warn(w) => warnings.push(w),
                RouteVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
            }
        }
        if let Some(e) = &p.endpoint {
            if let Ok(ip) = e.host.parse::<IpAddr>() {
                match check_endpoint(ip, &rctx) {
                    EndpointVerdict::Ok => {}
                    EndpointVerdict::NeedsConfirm(w) => warnings.push(w),
                    EndpointVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
                }
            }
        }
        let has_preshared = p.preshared.is_some();
        if let Some(s) = p.preshared {
            secrets.preshared.push((idx, s));
        }
        out_peers.push(WgPeer { public_key, endpoint: p.endpoint, allowed_ips: dedup(p.allowed), keepalive: p.keepalive, has_preshared });
    }

    let cfg = WgConfig { addresses, dns, mtu, listen_port, peers: out_peers };
    if cfg.full_tunnel() && cfg.dns.is_empty() {
        warnings.push("this is a full tunnel without DNS servers: name lookups go to your normal servers outside the tunnel".to_string());
    }
    if !cfg.peers.iter().any(|p| p.endpoint.is_some()) {
        warnings.push("no peer has an Endpoint: this computer can only wait for the other side to connect".to_string());
    }
    Ok(Imported { name, body: Body::Wireguard(cfg), secrets, warnings })
}

fn dedup(v: Vec<Cidr>) -> Vec<Cidr> {
    let mut out: Vec<Cidr> = Vec::new();
    for c in v {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}
