//! Running an OpenVPN profile (F-VPN-06): the fixed argument list of the program, reading what the server pushed, turning it into
//! the route and DNS plans IYAKKI applies itself, and classifying the program's log lines. Pure text work: starting the program is
//! `iyakki-agent::vpn_openvpn`. The program is told never to run a script (`--script-security 0`), never to touch addresses and
//! routes (`--ifconfig-noexec`, `--route-noexec`) and never to keep a password (`--auth-nocache`); what the server pushes is only
//! ever DATA that passes the same route and DNS checks as a WireGuard profile.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use crate::error::VpnError;
use crate::net::{check_dns, Cidr, Context};

/// The most routes a server may push before the push is refused (a hostile server cannot make the machine build a huge table).
pub const MAX_PUSHED_ROUTES: usize = 256;
const MAX_LINE: usize = 8192;

/// The arguments of the OpenVPN program for a profile: a fixed list, never a shell line. `config` is a private file that holds the
/// profile text with its secret blocks put back; `ifname` is the tunnel interface IYAKKI names.
pub fn command_args(config: &Path, ifname: &str) -> Result<Vec<String>, VpnError> {
    if !crate::wgnl::valid_ifname(ifname) {
        return Err(VpnError::Unsafe("not a valid interface name".into()));
    }
    let cfg = config.to_str().ok_or_else(|| VpnError::Unsafe("the config path is not text".into()))?;
    if cfg.contains(['\n', '\0']) {
        return Err(VpnError::Unsafe("the config path is not a plain path".into()));
    }
    let mut v: Vec<String> = Vec::new();
    for a in [
        "--config", cfg, "--dev", ifname, "--dev-type", "tun", "--script-security", "0", "--ifconfig-noexec", "--route-noexec", "--auth-nocache", "--verb", "4", "--suppress-timestamps", "--connect-retry-max", "3", "--ping-exit", "120", "--pull-filter", "ignore",
        "script-security", "--pull-filter", "ignore", "up", "--pull-filter", "ignore", "plugin",
    ] {
        v.push(a.to_string());
    }
    Ok(v)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pushed {
    /// The tunnel's own address (`ifconfig`, `ifconfig-ipv6`).
    pub addresses: Vec<Cidr>,
    pub routes: Vec<Cidr>,
    pub dns: Vec<IpAddr>,
    /// The server asked for all traffic (`redirect-gateway`).
    pub redirect_default: bool,
    pub mtu: Option<u16>,
    /// Names (never values) of pushed options that were not used.
    pub ignored: Vec<String>,
}

fn inv(reason: &str) -> VpnError {
    VpnError::Invalid { line: 0, directive: "server settings".into(), reason: reason.into() }
}

fn mask_to_prefix(m: Ipv4Addr) -> Option<u8> {
    let b = u32::from(m);
    let ones = b.leading_ones();
    (b.checked_shl(ones).unwrap_or(0) == 0).then_some(ones as u8)
}

fn v4(s: &str) -> Option<Ipv4Addr> {
    s.parse().ok()
}

/// Read one `PUSH_REPLY,opt,opt,...` text (what the server sent). Anything malformed is refused with the reason; unknown options are
/// ignored by name only.
pub fn parse_push_reply(text: &str) -> Result<Pushed, VpnError> {
    if text.len() > MAX_LINE || text.contains(['\0', '\n']) {
        return Err(VpnError::Unsafe("the server's settings are too long or not text".into()));
    }
    let text = text.trim().trim_matches('\'');
    let body = text.strip_prefix("PUSH_REPLY").ok_or_else(|| inv("not a PUSH_REPLY"))?;
    let mut out = Pushed::default();
    for opt in body.split(',').map(str::trim).filter(|o| !o.is_empty()) {
        let w: Vec<&str> = opt.split_whitespace().collect();
        match w.as_slice() {
            ["ifconfig", ip, second, ..] => {
                let ip = v4(ip).ok_or_else(|| inv("the tunnel address is not an IPv4 address"))?;
                // topology subnet: a netmask; net30 and p2p: the peer's address (a /32 for us)
                let prefix = v4(second).and_then(mask_to_prefix).unwrap_or(32);
                out.addresses.push(Cidr::new(IpAddr::V4(ip), prefix).ok_or_else(|| inv("bad tunnel address"))?);
            }
            ["ifconfig-ipv6", a, ..] => {
                let c = Cidr::parse(a).ok_or_else(|| inv("the tunnel IPv6 address is not valid"))?;
                out.addresses.push(c);
            }
            ["route", net, rest @ ..] => {
                let mask = rest.first().and_then(|m| v4(m)).and_then(mask_to_prefix).or(if rest.is_empty() { Some(32) } else { None }).ok_or_else(|| inv("a pushed route has no valid netmask"))?;
                let net = v4(net).ok_or_else(|| inv("a pushed route is not an IPv4 network"))?;
                out.routes.push(Cidr::new(IpAddr::V4(net), mask).ok_or_else(|| inv("a pushed route is not valid"))?.network());
            }
            ["route-ipv6", c, ..] => {
                out.routes.push(Cidr::parse(c).ok_or_else(|| inv("a pushed IPv6 route is not valid"))?.network());
            }
            ["dhcp-option", "DNS" | "DNS6", ip] => {
                out.dns.push(ip.parse().map_err(|_| inv("a pushed DNS server is not an address"))?);
            }
            ["redirect-gateway", ..] | ["redirect-private", ..] => out.redirect_default = true,
            ["tun-mtu", n] => {
                let n: u16 = n.parse().map_err(|_| inv("a pushed MTU is not a number"))?;
                if !(576..=9000).contains(&n) {
                    return Err(VpnError::Unsafe("a pushed MTU is outside 576 to 9000".into()));
                }
                out.mtu = Some(n);
            }
            [name, ..] => {
                let shown: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(32).collect();
                if !shown.is_empty() && !out.ignored.contains(&shown) {
                    out.ignored.push(shown);
                }
            }
            [] => {}
        }
        if out.routes.len() > MAX_PUSHED_ROUTES {
            return Err(VpnError::Unsafe(format!("the server pushed more than {MAX_PUSHED_ROUTES} routes")));
        }
    }
    if out.dns.len() > 8 {
        out.dns.truncate(8);
    }
    Ok(out)
}

/// What to apply, after the checks. The routes go through `plan::plan_routes` (the allowed networks), the DNS through `check_dns`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Checked {
    pub addresses: Vec<Cidr>,
    /// The networks to send through the tunnel (all of them when the profile or the server asks for the default route).
    pub allowed: Vec<Cidr>,
    pub dns: Vec<IpAddr>,
    pub warnings: Vec<String>,
    pub mtu: Option<u16>,
}

/// Combine the profile's own static routes and the pushed ones. `full` is true when the whole machine goes through the tunnel; then
/// a pushed `redirect-gateway` and the profile's both give the default route. DNS only counts for a full tunnel (as for WireGuard).
pub fn check_pushed(p: &Pushed, profile_routes: &[Cidr], profile_redirect: bool, full: bool, ctx: &Context) -> Result<Checked, VpnError> {
    if p.addresses.is_empty() {
        return Err(inv("the server did not give the tunnel an address"));
    }
    let mut allowed: Vec<Cidr> = profile_routes.to_vec();
    for r in &p.routes {
        if !allowed.contains(r) {
            allowed.push(*r);
        }
    }
    if full && (p.redirect_default || profile_redirect) {
        for d in [Cidr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).unwrap_or_else(|| p.addresses[0])] {
            if !allowed.contains(&d) {
                allowed.push(d);
            }
        }
    }
    if full && allowed.is_empty() {
        return Err(inv("the server pushed no routes, so nothing would go through the tunnel"));
    }
    let mut warnings = Vec::new();
    let dns = if full && !p.dns.is_empty() {
        let (d, w) = check_dns(&p.dns.iter().map(ToString::to_string).collect::<Vec<_>>(), ctx)?;
        warnings.extend(w);
        d
    } else {
        Vec::new()
    };
    Ok(Checked { addresses: p.addresses.clone(), allowed, dns, warnings, mtu: p.mtu })
}

/// What one log line of the program means (the text of a line is never kept beyond what is named here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The server's settings (`PUSH_REPLY,...`).
    Push(String),
    /// "Initialization Sequence Completed": the tunnel carries traffic.
    Up,
    /// The server refused the user name or password.
    AuthFailed,
    /// The TLS handshake failed or the certificate was refused.
    TlsFailed,
    /// The program is giving up (a fatal error, a ping timeout or a signal).
    Ended,
    /// Restarting after a lost connection.
    Restarting,
    Other,
}

pub fn classify_line(line: &str) -> Event {
    let l = line.trim();
    if l.len() > MAX_LINE {
        return Event::Other;
    }
    if let Some(i) = l.find("PUSH_REPLY") {
        if l.contains("PUSH: Received control message") {
            return Event::Push(l[i..].trim_end_matches('\'').to_string());
        }
    }
    if l.contains("Initialization Sequence Completed") && !l.contains("With Errors") {
        Event::Up
    } else if l.contains("AUTH_FAILED") || l.contains("auth-failure") {
        Event::AuthFailed
    } else if l.contains("TLS Error") || l.contains("VERIFY ERROR") || l.contains("TLS handshake failed") {
        Event::TlsFailed
    } else if l.contains("Exiting due to fatal error") || l.contains("SIGTERM[") && l.contains("exiting") || l.contains("Inactivity timeout (--ping-exit)") {
        Event::Ended
    } else if l.contains("SIGUSR1[soft") || l.contains("SIGUSR1[hard") {
        Event::Restarting
    } else {
        Event::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context::default()
    }

    #[test]
    fn the_arguments_are_a_fixed_list_that_forbids_scripts_and_file_writes() {
        let a = command_args(&PathBuf::from("/run/iyakki/vpn/p.ovpn"), "iyvpn1").unwrap();
        let j = a.join(" ");
        assert!(j.contains("--script-security 0") && j.contains("--ifconfig-noexec") && j.contains("--route-noexec") && j.contains("--auth-nocache"));
        assert!(!j.contains("--management") && !j.contains("--up ") && !j.contains("--log"));
        assert_eq!(a[0], "--config");
        assert!(command_args(&PathBuf::from("/x\ny"), "iyvpn1").is_err());
        assert!(command_args(&PathBuf::from("/x"), "bad name; rm").is_err());
    }

    #[test]
    fn a_normal_push_reply_is_read() {
        let p = parse_push_reply("PUSH_REPLY,route 10.8.0.0 255.255.255.0,route 192.168.50.0 255.255.255.0 net_gateway,dhcp-option DNS 10.8.0.1,dhcp-option DOMAIN corp,redirect-gateway def1,tun-mtu 1400,ifconfig 10.8.0.6 255.255.255.0,peer-id 3,cipher AES-256-GCM").unwrap();
        assert_eq!(p.addresses, vec![Cidr::parse("10.8.0.6/24").unwrap()]);
        assert_eq!(p.routes, vec![Cidr::parse("10.8.0.0/24").unwrap(), Cidr::parse("192.168.50.0/24").unwrap()]);
        assert_eq!(p.dns, vec!["10.8.0.1".parse::<IpAddr>().unwrap()]);
        assert!(p.redirect_default);
        assert_eq!(p.mtu, Some(1400));
        assert!(p.ignored.contains(&"peer-id".to_string()) && p.ignored.contains(&"cipher".to_string()));
    }

    #[test]
    fn net30_style_and_ipv6_pushes_are_read_and_bad_ones_refused() {
        let p = parse_push_reply("PUSH_REPLY,ifconfig 10.8.0.6 10.8.0.5,ifconfig-ipv6 fd00::2/64 fd00::1,route-ipv6 2001:db8::/32").unwrap();
        assert_eq!(p.addresses[0], Cidr::parse("10.8.0.6/32").unwrap());
        assert_eq!(p.addresses[1], Cidr::parse("fd00::2/64").unwrap());
        assert_eq!(p.routes, vec![Cidr::parse("2001:db8::/32").unwrap()]);
        for bad in ["hello", "PUSH_REPLY,route 10.0.0.0 255.0.255.0", "PUSH_REPLY,route notanip 255.0.0.0", "PUSH_REPLY,dhcp-option DNS banana", "PUSH_REPLY,tun-mtu 20", "PUSH_REPLY,ifconfig x 255.0.0.0", "PUSH_REPLY,route 10.0.0.0\n"] {
            assert!(parse_push_reply(bad).is_err(), "{bad}");
        }
        let many: String = (0..300).map(|i| format!(",route 10.{}.{}.0 255.255.255.0", i / 256, i % 256)).collect();
        assert!(parse_push_reply(&format!("PUSH_REPLY{many}")).is_err(), "too many routes");
        assert!(parse_push_reply(&format!("PUSH_REPLY,{}", "a".repeat(9000))).is_err());
    }

    #[test]
    fn unknown_pushed_options_are_named_only_and_never_run() {
        let p = parse_push_reply("PUSH_REPLY,up /bin/sh -c evil,script-security 2,ifconfig 10.8.0.2 255.255.255.0,route 10.9.0.0 255.255.0.0").unwrap();
        assert!(p.ignored.contains(&"up".to_string()) && p.ignored.contains(&"script-security".to_string()));
        assert!(!format!("{p:?}").contains("evil"), "the values are never kept");
    }

    #[test]
    fn what_is_pushed_goes_through_the_same_checks_as_a_wireguard_profile() {
        let p = parse_push_reply("PUSH_REPLY,ifconfig 10.8.0.6 255.255.255.0,redirect-gateway def1,dhcp-option DNS 10.8.0.1").unwrap();
        let full = check_pushed(&p, &[], false, true, &ctx()).unwrap();
        assert!(full.allowed.iter().any(Cidr::is_default));
        assert_eq!(full.dns.len(), 1);
        // a hybrid profile does not take the default route or the DNS of the server
        let hybrid = check_pushed(&p, &[Cidr::parse("10.8.0.0/24").unwrap()], false, false, &ctx()).unwrap();
        assert!(!hybrid.allowed.iter().any(Cidr::is_default) && hybrid.dns.is_empty());
        // no address, or a full tunnel with nothing to route
        assert!(check_pushed(&Pushed::default(), &[], false, true, &ctx()).is_err());
        let bare = parse_push_reply("PUSH_REPLY,ifconfig 10.8.0.6 255.255.255.0").unwrap();
        assert!(check_pushed(&bare, &[], false, true, &ctx()).is_err());
        // the profile's own redirect-gateway counts when the server pushes none
        assert!(check_pushed(&bare, &[], true, true, &ctx()).unwrap().allowed.iter().any(Cidr::is_default));
    }

    #[test]
    fn log_lines_are_classified() {
        let push = "PUSH: Received control message: 'PUSH_REPLY,route 10.8.0.0 255.255.255.0,ifconfig 10.8.0.2 255.255.255.0'";
        assert!(matches!(classify_line(push), Event::Push(t) if t.starts_with("PUSH_REPLY,route") && !t.ends_with('\'')));
        assert_eq!(classify_line("Initialization Sequence Completed"), Event::Up);
        assert_eq!(classify_line("Initialization Sequence Completed With Errors"), Event::Other);
        assert_eq!(classify_line("AUTH: Received control message: AUTH_FAILED"), Event::AuthFailed);
        assert_eq!(classify_line("TLS Error: TLS key negotiation failed to occur within 60 seconds"), Event::TlsFailed);
        assert_eq!(classify_line("VERIFY ERROR: depth=0, error=certificate has expired"), Event::TlsFailed);
        assert_eq!(classify_line("Exiting due to fatal error"), Event::Ended);
        assert_eq!(classify_line("SIGUSR1[soft,ping-restart] received, process restarting"), Event::Restarting);
        assert_eq!(classify_line("some other line"), Event::Other);
        assert_eq!(classify_line(&"x".repeat(9000)), Event::Other);
    }
}
