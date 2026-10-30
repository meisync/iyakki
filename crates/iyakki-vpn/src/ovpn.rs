//! `.ovpn` import. A strict **allow-list**: a directive that is not listed is refused, not ignored. Directives that run
//! programs, read or write files, open control sockets or take over what IYAKKI sets itself are refused with the reason
//! (threat list, "Malicious .ovpn runs commands"). Inline blocks are limited to the known names; the ones that hold secrets
//! (`key`, `tls-auth`, `tls-crypt`, `tls-crypt-v2`, `pkcs12`) are cut out into [`ProfileSecrets`].
//!
//! Starting the program is a later step; this only decides what may be written to its config file.

use std::net::{IpAddr, Ipv4Addr};

use crate::error::{invalid, rejected, VpnError};
use crate::net::{check_dns, check_endpoint, check_route, Cidr, Context, EndpointVerdict, RouteVerdict};
use crate::profile::{Body, Imported, OvpnConfig, OvpnRemote};
use crate::secret::{ProfileSecrets, Secret};
use crate::text;
use crate::wgconf::shown;

const PUBLIC_BLOCKS: [&str; 4] = ["ca", "cert", "dh", "extra-certs"];
const SECRET_BLOCKS: [&str; 5] = ["key", "tls-auth", "tls-crypt", "tls-crypt-v2", "pkcs12"];
const PROTOS: [&str; 9] = ["udp", "tcp", "udp4", "udp6", "tcp4", "tcp6", "tcp-client", "tcp4-client", "tcp6-client"];

/// Every directive the importer accepts (the arms of the match in [`parse`]); a test keeps this list in step with the match.
pub const ACCEPTED_DIRECTIVES: &[&str] = &[
    "client", "tls-client", "pull", "nobind", "persist-key", "persist-tun", "remote-random", "remote-random-hostname", "float", "auth-nocache", "route-nopull", "push-peer-info", "mute-replay-warnings", "tun-ipv6", "fast-io",
    "script-security", "block-outside-dns", "register-dns", "dev", "dev-type", "proto", "remote", "remote-cert-tls", "ns-cert-type", "verify-x509-name", "cipher", "data-ciphers", "data-ciphers-fallback", "ncp-ciphers",
    "tls-cipher", "tls-ciphersuites", "auth", "tls-version-min", "tls-version-max", "verb", "mute", "reneg-sec", "tran-window", "connect-retry-max", "server-poll-timeout", "ping", "ping-restart", "ping-exit", "hand-window",
    "tun-mtu", "sndbuf", "rcvbuf", "connect-timeout", "reneg-bytes", "connect-retry", "keepalive", "mssfix", "fragment", "explicit-exit-notify", "key-direction", "resolv-retry", "auth-retry", "auth-user-pass", "setenv",
    "setenv-safe", "pull-filter", "redirect-gateway", "route", "route-ipv6", "dhcp-option",
];

/// Is this a directive name the importer knows (accepted, ignored or refused with a reason)? Used to decide which names may be
/// printed by `iyakki vpn inspect`; a name outside the list could be anything, including a pasted secret.
pub fn known_directive(name: &str) -> bool {
    ACCEPTED_DIRECTIVES.contains(&name) || IGNORED_DIRECTIVES.iter().any(|(n, _)| *n == name) || refusal(name).is_some()
}

fn refusal(name: &str) -> Option<&'static str> {
    Some(match name {
        "up" | "down" | "route-up" | "route-pre-down" | "ipchange" | "tls-verify" | "auth-user-pass-verify" | "client-connect" | "client-disconnect" | "learn-address" | "up-restart" | "down-pre" => {
            "it runs a program as root"
        }
        "plugin" | "engine" | "providers" => "it loads code into the VPN program",
        "ifconfig-noexec" | "route-noexec" => "IYAKKI applies addresses and routes itself and sets this",
        "echo" | "daemon" | "log" | "log-append" | "writepid" | "status" | "syslog" | "tmp-dir" | "dev-node" | "persist-tun-dir" => "it chooses a file to write or how the program runs",
        "chroot" | "cd" | "user" | "group" => "IYAKKI sets this itself",
        "config" => "it includes another file",
        "askpass" => "it reads a password from a file",
        "ca" | "cert" | "key" | "dh" | "tls-auth" | "tls-crypt" | "tls-crypt-v2" | "pkcs12" | "extra-certs" | "crl-verify" | "secret" | "http-proxy-user-pass" | "capath" => {
            "it names a file on this computer; put the content inline in a <ca>-style block instead"
        }
        "compress" | "comp-lzo" | "comp-noadapt" => "compression allows the VORACLE attack",
        "ignore-unknown-option" => "it hides directives that would otherwise be checked",
        "connection" => "per-server connection blocks are not supported",
        _ if name.starts_with("management") => "it opens a local control socket that can read secrets and run commands",
        _ => return None,
    })
}

/// Directives that provider files carry and that do nothing harmful or useful on these images. They are **ignored with a visible
/// note**, not refused. The form must match exactly (name and the arguments shown); anything else with the same name goes through
/// the normal rules (so `comp-lzo yes` is still refused). Documented in `docs/development/VPN-CONNECTOR-PLAN.md`.
pub const IGNORED_DIRECTIVES: &[(&str, &str)] = &[
    ("key-method", "2"),
    ("tls-timeout", "<number>"),
    ("disable-occ", ""),
    ("suppress-timestamps", ""),
    ("ifconfig-nowarn", ""),
    ("windows-driver", "<word>"),
    ("route-delay", "<number> [<number>]"),
    ("route-metric", "<number>"),
    ("tun-mtu-extra", "<number>"),
    ("mlock", ""),
    ("mtu-disc", "no|maybe|yes"),
    ("allow-compression", "no"),
    ("comp-lzo", "no"),
    ("setenv", "opt block-outside-dns"),
    ("ignore-unknown-option", "block-outside-dns"),
];

fn is_ignored(name: &str, args: &[String]) -> bool {
    let num = |a: &String| !a.is_empty() && a.len() <= 9 && a.bytes().all(|b| b.is_ascii_digit());
    match name {
        "key-method" => args == ["2"],
        "tls-timeout" | "route-metric" | "tun-mtu-extra" => args.len() == 1 && num(&args[0]),
        "disable-occ" | "suppress-timestamps" | "ifconfig-nowarn" | "mlock" => args.is_empty(),
        "windows-driver" => args.len() == 1,
        "route-delay" => (1..=2).contains(&args.len()) && args.iter().all(num),
        "mtu-disc" => args.len() == 1 && ["no", "maybe", "yes"].contains(&args[0].as_str()),
        "allow-compression" | "comp-lzo" => args == ["no"],
        "setenv" => args == ["opt", "block-outside-dns"],
        "ignore-unknown-option" => args == ["block-outside-dns"],
        _ => false,
    }
}

/// Split a line into words. Double or single quotes only around a whole word; a backslash is never accepted.
pub(crate) fn tokenize(line: &str) -> Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut tok = String::new();
        if c == '"' || c == '\'' {
            chars.next();
            loop {
                match chars.next() {
                    None => return Err("a quote is not closed"),
                    Some(q) if q == c => break,
                    Some('\\') => return Err("a backslash is not accepted"),
                    Some(x) => tok.push(x),
                }
            }
            if tok.is_empty() {
                return Err("an empty word");
            }
            if chars.peek().is_some_and(|n| !n.is_whitespace()) {
                return Err("a quote in the middle of a word");
            }
        } else {
            while let Some(&x) = chars.peek() {
                if x.is_whitespace() {
                    break;
                }
                if x == '"' || x == '\'' || x == '\\' {
                    return Err("a quote or backslash in the middle of a word");
                }
                tok.push(x);
                chars.next();
            }
        }
        out.push(tok);
    }
    Ok(out)
}

fn plain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:/+=,@%-[]!".contains(&b))
}

fn nums(args: &[String], count: std::ops::RangeInclusive<usize>, max: u32) -> bool {
    count.contains(&args.len()) && args.iter().all(|a| !a.is_empty() && a.len() <= 9 && a.bytes().all(|b| b.is_ascii_digit()) && a.parse::<u32>().is_ok_and(|n| n <= max))
}

/// Dotted netmask to prefix length; `None` when it is not contiguous.
fn mask_prefix(m: &str) -> Option<u8> {
    let v = u32::from(m.parse::<Ipv4Addr>().ok()?);
    let p = v.leading_ones();
    (v.checked_shl(p).unwrap_or(0) == 0).then_some(p as u8)
}

fn parse_route(args: &[String]) -> Option<Cidr> {
    let net = args.first()?;
    let addr: IpAddr = net.parse().ok().filter(IpAddr::is_ipv4)?;
    let prefix = match args.get(1).map(String::as_str) {
        None => 32,
        Some("default") => 32,
        Some(m) => mask_prefix(m)?,
    };
    Some(Cidr::new(addr, prefix)?.network())
}

struct State {
    out: String,
    remotes: Vec<OvpnRemote>,
    routes: Vec<Cidr>,
    dns: Vec<String>,
    redirect: bool,
    needs_credentials: bool,
    warnings: Vec<String>,
}

/// Parse and sanitise a `.ovpn` file.
pub fn parse(name: &str, input: &[u8], ctx: &Context) -> Result<Imported, VpnError> {
    let name = text::check_name(name)?;
    let src = text::clean(input)?;
    let mut st = State { out: String::new(), remotes: Vec::new(), routes: Vec::new(), dns: Vec::new(), redirect: false, needs_credentials: false, warnings: Vec::new() };
    let mut secrets = ProfileSecrets::default();
    let mut secret_names: Vec<String> = Vec::new();
    let mut seen_blocks: Vec<String> = Vec::new();
    let mut block: Option<(String, usize, String)> = None;

    for (i, raw) in src.split('\n').enumerate() {
        let ln = i + 1;
        let line = raw.trim();
        if let Some((bname, start, body)) = block.as_mut() {
            if line == format!("</{bname}>") {
                let (bname, _, body) = block.take().unwrap();
                if SECRET_BLOCKS.contains(&bname.as_str()) {
                    secret_names.push(bname.clone());
                    secrets.blocks.push((bname, Secret::new(body.into_bytes())));
                } else {
                    st.out.push_str(&format!("<{bname}>\n{body}</{bname}>\n"));
                }
            } else if line.starts_with('<') {
                return Err(invalid(*start, bname, "a block inside a block, or a block that is not closed"));
            } else {
                if !line.is_ascii() {
                    return Err(invalid(ln, bname, "a block may hold only plain ASCII text"));
                }
                if body.len() + line.len() + 1 > text::MAX_BLOCK {
                    return Err(VpnError::TooLarge(format!("the <{bname}> block is larger than {} KB", text::MAX_BLOCK / 1024)));
                }
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('<') {
            let Some(bname) = rest.strip_suffix('>') else {
                return Err(invalid(ln, "block", "a block tag must be alone on its line"));
            };
            if !PUBLIC_BLOCKS.contains(&bname) && !SECRET_BLOCKS.contains(&bname) {
                return Err(rejected(ln, shown(bname), "this block is not supported"));
            }
            if seen_blocks.iter().any(|b| b == bname) {
                return Err(invalid(ln, bname, "given twice"));
            }
            seen_blocks.push(bname.to_string());
            block = Some((bname.to_string(), ln, String::new()));
            continue;
        }

        let toks = tokenize(line).map_err(|why| invalid(ln, "line", why))?;
        let Some(first) = toks.first() else { continue };
        let dname = first.strip_prefix("--").unwrap_or(first);
        let d = shown(dname);
        let args = &toks[1..];

        if args.iter().any(|a| !plain(a)) {
            return Err(invalid(ln, d, "a value has characters that are not accepted"));
        }
        if is_ignored(dname, args) {
            st.warnings.push(format!("ignored: \"{d}\" (harmless on these images)"));
            continue;
        }
        if let Some(why) = refusal(dname) {
            return Err(rejected(ln, d, why));
        }
        let bad = |why: &str| invalid(ln, d, why);

        let keep = match dname {
            "client" | "tls-client" | "pull" | "nobind" | "persist-key" | "persist-tun" | "remote-random" | "remote-random-hostname" | "float" | "auth-nocache" | "route-nopull" | "push-peer-info"
            | "mute-replay-warnings" | "tun-ipv6" | "fast-io" => {
                if !args.is_empty() {
                    return Err(bad("takes no value"));
                }
                true
            }
            "script-security" => {
                if args.len() != 1 || args[0] != "0" {
                    return Err(rejected(ln, d, "any value above 0 lets the profile run programs; IYAKKI always uses 0"));
                }
                false
            }
            "block-outside-dns" | "register-dns" => {
                st.warnings.push(format!("\"{d}\" is for Windows and was ignored"));
                false
            }
            "dev" => {
                if args.len() != 1 || args[0] != "tun" {
                    return Err(rejected(ln, d, "only layer-3 tunnels (dev tun) are supported"));
                }
                true
            }
            "dev-type" => {
                if args.len() != 1 || args[0] != "tun" {
                    return Err(rejected(ln, d, "only dev-type tun is supported"));
                }
                true
            }
            "proto" => {
                if args.len() != 1 || !PROTOS.contains(&args[0].as_str()) {
                    return Err(bad("expected udp or tcp (client forms)"));
                }
                true
            }
            "remote" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(bad("expected host [port [proto]]"));
                }
                let host = &args[0];
                let ip = host.parse::<IpAddr>().ok();
                if ip.is_none() && (!text::valid_hostname(host) || host.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))) {
                    return Err(bad("the server must be a host name or an address"));
                }
                let port = match args.get(1) {
                    Some(p) => p.parse::<u16>().ok().filter(|&p| p != 0).ok_or_else(|| bad("not a port number"))?,
                    None => 1194,
                };
                let proto = match args.get(2) {
                    Some(p) if PROTOS.contains(&p.as_str()) => Some(p.clone()),
                    Some(_) => return Err(bad("expected udp or tcp")),
                    None => None,
                };
                if let Some(ip) = ip {
                    match check_endpoint(ip, ctx) {
                        EndpointVerdict::Ok => {}
                        EndpointVerdict::NeedsConfirm(w) => st.warnings.push(w),
                        EndpointVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
                    }
                }
                st.remotes.push(OvpnRemote { host: host.clone(), port, proto });
                true
            }
            "remote-cert-tls" | "ns-cert-type" => {
                if args != ["server"] {
                    return Err(bad("only \"server\" is accepted"));
                }
                true
            }
            "verify-x509-name" => {
                if args.is_empty() || args.len() > 2 || args.get(1).is_some_and(|t| !["name", "name-prefix", "subject"].contains(&t.as_str())) {
                    return Err(bad("expected name [name|name-prefix|subject]"));
                }
                true
            }
            "cipher" | "data-ciphers" | "data-ciphers-fallback" | "ncp-ciphers" | "tls-cipher" | "tls-ciphersuites" | "auth" => {
                if args.len() != 1 {
                    return Err(bad("expected one value"));
                }
                if args[0].split([':', ',']).any(|c| c.eq_ignore_ascii_case("none")) {
                    return Err(rejected(ln, d, "\"none\" would turn encryption or authentication off"));
                }
                true
            }
            "tls-version-min" | "tls-version-max" => {
                if args.is_empty() || args.len() > 2 || !["1.0", "1.1", "1.2", "1.3"].contains(&args[0].as_str()) || args.get(1).is_some_and(|a| a != "or-highest") {
                    return Err(bad("expected 1.0, 1.1, 1.2 or 1.3"));
                }
                if dname == "tls-version-min" && ["1.0", "1.1"].contains(&args[0].as_str()) {
                    st.warnings.push("this profile allows TLS older than 1.2".to_string());
                }
                true
            }
            "verb" | "mute" | "reneg-sec" | "tran-window" | "connect-retry-max" | "server-poll-timeout" | "ping" | "ping-restart" | "ping-exit" | "hand-window" | "tun-mtu" | "sndbuf" | "rcvbuf"
            | "connect-timeout" | "reneg-bytes" => {
                if !nums(args, 1..=1, 1 << 30) {
                    return Err(bad("expected one number"));
                }
                true
            }
            "connect-retry" | "keepalive" => {
                if !nums(args, 1..=2, 1 << 30) {
                    return Err(bad("expected numbers"));
                }
                true
            }
            "mssfix" | "fragment" | "explicit-exit-notify" => {
                if !nums(args, 0..=1, 65535) {
                    return Err(bad("expected a number"));
                }
                true
            }
            "key-direction" => {
                if args.len() != 1 || !["0", "1"].contains(&args[0].as_str()) {
                    return Err(bad("expected 0 or 1"));
                }
                true
            }
            "resolv-retry" => {
                if args.len() != 1 || (args[0] != "infinite" && !nums(args, 1..=1, 1 << 30)) {
                    return Err(bad("expected infinite or a number"));
                }
                true
            }
            "auth-retry" => {
                if args.len() != 1 || !["none", "nointeract"].contains(&args[0].as_str()) {
                    return Err(rejected(ln, d, "\"interact\" would ask on a terminal; use none or nointeract"));
                }
                true
            }
            "auth-user-pass" => {
                if !args.is_empty() {
                    return Err(rejected(ln, d, "it names a file; IYAKKI asks for the user name and password and supplies them itself"));
                }
                st.needs_credentials = true;
                false
            }
            "setenv" | "setenv-safe" => {
                if args.len() != 2 || args[0] != "CLIENT_CERT" || !["0", "1"].contains(&args[1].as_str()) {
                    return Err(rejected(ln, d, "only \"setenv CLIENT_CERT 0\" is accepted; other variables could change how programs behave"));
                }
                true
            }
            "pull-filter" => {
                if args.len() != 2 || !["ignore", "reject"].contains(&args[0].as_str()) {
                    return Err(rejected(ln, d, "only ignore and reject filters are accepted"));
                }
                true
            }
            "redirect-gateway" => {
                if args.iter().any(|a| !["local", "autolocal", "def1", "bypass-dhcp", "bypass-dns", "ipv6", "!ipv4"].contains(&a.as_str())) {
                    return Err(bad("unknown flag"));
                }
                st.redirect = true;
                false
            }
            "route" => {
                if args.is_empty() || args.len() > 4 {
                    return Err(bad("expected network [netmask [gateway [metric]]]"));
                }
                let c = parse_route(args).ok_or_else(|| bad("expected a network address and a netmask"))?;
                if c.is_default() {
                    st.redirect = true;
                } else {
                    st.routes.push(c);
                }
                false
            }
            "route-ipv6" => {
                let c = args.first().and_then(|a| Cidr::parse(a)).filter(|c| !c.is_v4() && args.len() <= 3).ok_or_else(|| bad("expected an IPv6 network with a prefix"))?;
                if c.is_default() {
                    st.redirect = true;
                } else {
                    st.routes.push(c.network());
                }
                false
            }
            "dhcp-option" => {
                match args.first().map(String::as_str) {
                    Some("DNS") | Some("DNS6") if args.len() == 2 => st.dns.push(args[1].clone()),
                    Some(_) if args.len() >= 2 => st.warnings.push("a dhcp-option other than DNS was ignored".to_string()),
                    _ => return Err(bad("expected an option and a value")),
                }
                false
            }
            _ => return Err(rejected(ln, d, "not on the list of supported directives")),
        };
        if keep {
            st.out.push_str(dname);
            for a in args {
                st.out.push(' ');
                st.out.push_str(a);
            }
            st.out.push('\n');
        }
    }
    if let Some((bname, start, _)) = block {
        return Err(invalid(start, &bname, "the block is not closed"));
    }
    if st.remotes.is_empty() {
        return Err(VpnError::Missing("the profile has no \"remote\" server".into()));
    }
    if !seen_blocks.iter().any(|b| b == "ca") {
        return Err(VpnError::Missing("the profile has no inline <ca> block; the server could not be verified".into()));
    }
    if !st.out.lines().any(|l| l.starts_with("remote-cert-tls ") || l.starts_with("verify-x509-name ")) {
        st.warnings.push("the profile does not check the server certificate type (remote-cert-tls server)".to_string());
    }

    let (dns, dns_warn) = check_dns(&st.dns, ctx)?;
    st.warnings.extend(dns_warn);
    for r in &st.routes {
        match check_route(r, ctx) {
            RouteVerdict::Ok => {}
            RouteVerdict::Warn(w) => st.warnings.push(w),
            RouteVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
        }
    }
    if st.redirect && dns.is_empty() {
        st.warnings.push("this is a full tunnel without DNS servers in the profile: the server may push them, otherwise name lookups go outside the tunnel".to_string());
    }
    let cfg = OvpnConfig { text: st.out, remotes: st.remotes, routes: st.routes, dns, redirect_gateway: st.redirect, needs_credentials: st.needs_credentials, secret_blocks: secret_names };
    Ok(Imported { name, body: Body::Openvpn(cfg), secrets, warnings: st.warnings })
}

/// The config file text for the program: the sanitised text plus the secret blocks put back. Written to tmpfs by the helper.
pub fn assemble(cfg: &OvpnConfig, secrets: &ProfileSecrets) -> Result<String, VpnError> {
    let mut out = cfg.text.clone();
    for name in &cfg.secret_blocks {
        let (_, body) = secrets.blocks.iter().find(|(n, _)| n == name).ok_or_else(|| VpnError::Missing(format!("the secret block <{name}> is missing")))?;
        let body = std::str::from_utf8(body.expose()).map_err(|_| VpnError::BadText("a secret block is not text".into()))?;
        out.push_str(&format!("<{name}>\n{body}</{name}>\n"));
    }
    Ok(out)
}
